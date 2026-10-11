use super::*;
use std::sync::{atomic::AtomicBool, Condvar};

type Reply = (
    ImageSrc,
    std::result::Result<(egui::ColorImage, Option<[usize; 2]>), String>,
);
const FULL_BUDGET: usize = 512 * 1024 * 1024;
fn thumbnail(source: &ImageSrc) -> Result<(egui::ColorImage, Option<[usize; 2]>)> {
    match source {
        ImageSrc::File(path) => {
            let size = image::image_dimensions(path)
                .ok()
                .map(|(w, h)| [w as usize, h as usize]);
            Ok((decode_thumbnail(source, true)?, size))
        }
        ImageSrc::Entry { archive, name } => {
            // Read just this entry, in the worker. Keep references to the archive; never
            // extract into a user folder or persist a temporary path in the board.
            let bytes = read_archive_entry(archive, name)?;
            let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()?
                .into_dimensions()
                .ok()
                .map(|(w, h)| [w as usize, h as usize]);
            let epoch = thumbnail_cache::epoch();
            let cache = thumbnail_cache::key(source, &thumbnail_cache::directory()).ok();
            let rgba = match cache.as_ref().and_then(|p| thumbnail_cache::load(p)) {
                Some(rgba) => rgba,
                None => {
                    let rgba = image_decode::thumbnail(&bytes, 320, 240)?.to_rgba8();
                    if let Some(path) = &cache {
                        let _ = thumbnail_cache::save(path, &rgba, epoch);
                    }
                    rgba
                }
            };
            Ok((
                egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                ),
                size,
            ))
        }
    }
}
struct Job {
    source: ImageSrc,
    reply: mpsc::Sender<Reply>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
}
struct Workers {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
}
fn workers() -> &'static Arc<Workers> {
    static WORKERS: OnceLock<Arc<Workers>> = OnceLock::new();
    WORKERS.get_or_init(|| {
        let workers = Arc::new(Workers {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        for _ in 0..2 {
            let workers = workers.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let mut q = workers.queue.lock().unwrap();
                    while q.is_empty() {
                        q = workers.wake.wait(q).unwrap();
                    }
                    q.pop_front().unwrap()
                };
                if job.cancel.load(AtomicOrdering::Relaxed) {
                    continue;
                }
                let result = thumbnail(&job.source).map_err(|e| e.to_string());
                if !job.cancel.load(AtomicOrdering::Relaxed) {
                    let _ = job.reply.send((job.source, result));
                    job.ctx.request_repaint();
                }
            });
        }
        workers
    })
}
struct Asset {
    thumbnail: Option<media_gpu::Texture>,
    full: Option<media_gpu::Texture>,
    client: Option<image_pipeline::Client>,
    error: Option<String>,
    pending: bool,
    full_failed: bool,
    seen: u64,
    full_bytes: Option<usize>,
}
pub struct Assets {
    items: HashMap<ImageSrc, Asset>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    frame: u64,
}
impl Default for Assets {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            items: HashMap::new(),
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            frame: 0,
        }
    }
}
impl Drop for Assets {
    fn drop(&mut self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}
impl Assets {
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        for _ in 0..4 {
            let Ok((path, result)) = self.rx.try_recv() else {
                break;
            };
            if let Some(asset) = self.items.get_mut(&path) {
                asset.pending = false;
                match result {
                    Ok((image, size)) => {
                        asset.full_bytes = size.map(media_gpu::mip_bytes);
                        asset.thumbnail = Some(
                            ctx.load_texture("board-thumbnail", image, TextureOptions::LINEAR)
                                .into(),
                        )
                    }
                    Err(e) => asset.error = Some(e),
                }
            }
            ctx.request_repaint();
        }
        // Upload at most one full image per UI pass; other clients remain queued in the shared pool.
        for asset in self.items.values_mut() {
            if let Some(result) = asset.client.as_mut().and_then(|c| c.poll(ctx)) {
                match result {
                    Ok(texture) => {
                        asset.full_bytes = Some(media_gpu::mip_bytes(texture.size()));
                        asset.full = Some(texture);
                    }
                    Err(error) => {
                        asset.error = Some(error);
                        asset.full_failed = true;
                    }
                }
                asset.client = None;
                break;
            }
        }
    }
    pub fn get(
        &mut self,
        source: &ImageSrc,
        screen_size: Vec2,
        ctx: &egui::Context,
    ) -> (Option<media_gpu::Texture>, Option<String>) {
        if !self.items.contains_key(source) && self.items.len() >= 256 {
            if let Some(old) = self
                .items
                .iter()
                .filter(|(_, a)| a.seen + 1 < self.frame)
                .min_by_key(|(_, a)| a.seen)
                .map(|(p, _)| p.clone())
            {
                self.items.remove(&old);
            } else {
                return (None, None);
            }
        }
        let active = self.items.values().filter(|a| a.client.is_some()).count();
        let reserved: usize = self
            .items
            .values()
            .filter(|a| a.full.is_some() || a.client.is_some())
            .map(|a| a.full_bytes.unwrap_or(FULL_BUDGET))
            .sum();
        let asset = self.items.entry(source.clone()).or_insert_with(|| Asset {
            thumbnail: None,
            full: None,
            client: None,
            error: None,
            pending: false,
            full_failed: false,
            seen: 0,
            full_bytes: None,
        });
        asset.seen = self.frame;
        if asset.thumbnail.is_none() && asset.error.is_none() && !asset.pending {
            let mut q = workers().queue.lock().unwrap();
            q.retain(|j| !j.cancel.load(AtomicOrdering::Relaxed));
            if q.len() < 64 {
                q.push_back(Job {
                    source: source.clone(),
                    reply: self.tx.clone(),
                    cancel: self.cancel.clone(),
                    ctx: ctx.clone(),
                });
                asset.pending = true;
                workers().wake.notify_one();
            }
        }
        if screen_size.max_elem() > 450.0
            && asset.thumbnail.is_some()
            && asset.full.is_none()
            && asset.client.is_none()
            && !asset.full_failed
            && active < 2
        {
            let bytes = asset.full_bytes.unwrap_or(FULL_BUDGET);
            // Reserve before requesting a decode, including in-flight images. Otherwise a board
            // larger than the budget could upload and evict the same textures on every frame.
            if reserved.saturating_add(bytes) <= FULL_BUDGET {
                let mut client = image_pipeline::Client::default();
                match client.request(source.clone(), vec![], ctx) {
                    Ok(Some(t)) => asset.full = Some(t),
                    Ok(None) => asset.client = Some(client),
                    Err(e) => {
                        asset.error = Some(e.to_string());
                        asset.full_failed = true;
                    }
                }
            }
        }
        (
            asset.full.as_ref().or(asset.thumbnail.as_ref()).cloned(),
            asset.error.clone(),
        )
    }
    pub fn end(&mut self, ctx: &egui::Context) {
        // Visible full images have a separate hard holding budget; the existing shared LRU owns
        // reusable copies. This prevents a board from pinning every source raster indefinitely.
        let mut full: Vec<_> = self
            .items
            .iter()
            .filter_map(|(p, a)| {
                a.full
                    .as_ref()
                    .map(|t| (p.clone(), a.seen, media_gpu::mip_bytes(t.size())))
            })
            .collect();
        full.sort_by_key(|(_, seen, _)| std::cmp::Reverse(*seen));
        let mut used = 0;
        for (path, seen, bytes) in full {
            used += bytes;
            if seen != self.frame || used > FULL_BUDGET {
                self.items.get_mut(&path).unwrap().full = None;
                ctx.request_repaint();
            }
        }
        for a in self.items.values_mut() {
            if a.seen + 1 < self.frame {
                a.client = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_previews_keep_entry_identity_and_source_dimensions() {
        use std::io::Write;
        let root = super::super::tests::fixture("archive-previews");
        let archive = root.join("images.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for (name, color) in [
            ("a/cover.png", [240, 20, 30]),
            ("b/cover.png", [20, 220, 40]),
        ] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(48, 32, image::Rgb(color)))
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes.get_ref()).unwrap();
        }
        zip.finish().unwrap();
        let sources = ["a/cover.png", "b/cover.png"].map(|name| ImageSrc::Entry {
            archive: archive.clone(),
            name: name.into(),
        });
        for (i, source) in sources.iter().enumerate() {
            let (cold, size) = thumbnail(source).unwrap();
            let (warm, _) = thumbnail(source).unwrap();
            assert_eq!(size, Some([48, 32]));
            assert_eq!(cold.pixels, warm.pixels);
            assert_eq!(
                cold.pixels[0].to_array(),
                if i == 0 {
                    [240, 20, 30, 255]
                } else {
                    [20, 220, 40, 255]
                }
            );
        }
        let ctx = egui::Context::default();
        let mut assets = Assets::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assets.begin(&ctx);
            for source in &sources {
                assets.get(source, Vec2::splat(700.0), &ctx);
            }
            assets.end(&ctx);
            if sources
                .iter()
                .all(|s| assets.items.get(s).is_some_and(|a| a.full.is_some()))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Board archive previews and full images must load asynchronously"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let first = assets.items[&sources[0]].full.as_ref().unwrap();
        let second = assets.items[&sources[1]].full.as_ref().unwrap();
        assert_eq!(first.size(), [48, 32]);
        assert_ne!(first.id(), second.id());
        drop(assets);
        std::fs::remove_dir_all(root).unwrap();
    }
}
