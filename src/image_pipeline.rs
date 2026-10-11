use super::*;
use std::sync::{Condvar, OnceLock};

type Decoded = std::result::Result<Arc<media_gpu::Photo>, String>;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    entry: Option<String>,
    bytes: u64,
    modified: Option<std::time::SystemTime>,
}
impl Key {
    fn new(src: &ImageSrc) -> Result<Self> {
        let (path, entry) = match src {
            ImageSrc::File(p) => (p, None),
            ImageSrc::Entry { archive, name } => (archive, Some(name.clone())),
        };
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            path: path.clone(),
            entry,
            bytes: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}
struct Lru<T> {
    items: HashMap<Key, (T, usize, u64)>,
    used: usize,
    budget: usize,
    clock: u64,
}
impl<T: Clone> Lru<T> {
    fn new(budget: usize) -> Self {
        Self {
            items: HashMap::new(),
            used: 0,
            budget,
            clock: 0,
        }
    }
    fn get(&mut self, key: &Key) -> Option<T> {
        self.clock += 1;
        self.items.get_mut(key).map(|(v, _, t)| {
            *t = self.clock;
            v.clone()
        })
    }
    fn trim(&mut self) {
        while self.used > self.budget || self.items.len() > 128 {
            let Some(k) = self
                .items
                .iter()
                .min_by_key(|(_, (_, _, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.remove(&k);
        }
    }
    fn remove(&mut self, key: &Key) {
        if let Some((_, bytes, _)) = self.items.remove(key) {
            self.used -= bytes;
        }
    }
    fn put(&mut self, key: Key, value: T, bytes: usize) {
        self.remove(&key);
        if bytes > self.budget || self.budget == 0 {
            return;
        }
        self.clock += 1;
        self.used += bytes;
        self.items.insert(key, (value, bytes, self.clock));
        self.trim();
    }
    fn configure(&mut self, budget: usize) {
        self.budget = budget;
        self.trim();
    }
}
struct Job {
    key: Key,
    source: ImageSrc,
    token: Arc<AtomicU64>,
    generation: u64,
    reply: Option<Arc<Mutex<Option<(u64, Decoded)>>>>,
    ctx: egui::Context,
    epoch: u64,
}
struct Engine {
    ram: Mutex<Lru<Decoded>>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    epoch: AtomicU64,
}
fn engine() -> &'static Arc<Engine> {
    static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let engine = Arc::new(Engine {
            ram: Mutex::new(Lru::new(default_ram_mb() * 1024 * 1024)),
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            epoch: AtomicU64::new(0),
        });
        for _ in 0..2 {
            let engine = engine.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let mut q = engine.queue.lock().unwrap();
                    while q.is_empty() {
                        q = engine.wake.wait(q).unwrap();
                    }
                    q.pop_front().unwrap()
                };
                if job.token.load(AtomicOrdering::Relaxed) != job.generation {
                    continue;
                }
                let cached = engine.ram.lock().unwrap().get(&job.key);
                let mut result = cached.unwrap_or_else(|| {
                    decode(&job.source).map(Arc::new).map_err(|e| e.to_string())
                });
                // Already-decoded pixels remain useful in the bounded shared cache.
                // The reply below still checks the request generation.
                // File replacement during decode must not populate an old fingerprint.
                if Key::new(&job.source).ok().as_ref() != Some(&job.key) {
                    result = Err("File changed while decoding; select it again".into());
                }
                let bytes = result.as_ref().map_or(0, |image| image.bytes.len());
                if engine.epoch.load(AtomicOrdering::Relaxed) == job.epoch {
                    engine
                        .ram
                        .lock()
                        .unwrap()
                        .put(job.key, result.clone(), bytes);
                }
                if let Some(reply) = job.reply {
                    let mut reply = reply.lock().unwrap();
                    if job.token.load(AtomicOrdering::Relaxed) == job.generation {
                        *reply = Some((job.generation, result));
                    }
                }
                job.ctx.request_repaint();
            });
        }
        engine
    })
}
fn decode(source: &ImageSrc) -> Result<media_gpu::Photo> {
    let bytes = match source {
        ImageSrc::File(p) => std::fs::read(p)?,
        ImageSrc::Entry { archive, name } => read_archive_entry(archive, name)?,
    };
    let mut image = image_decode::decode(&bytes)?;
    if image.width() > MAX_TEX || image.height() > MAX_TEX {
        image = image.thumbnail(MAX_TEX, MAX_TEX);
    }
    Ok(media_gpu::Photo::from_image(image))
}
fn gpu(ctx: &egui::Context) -> Arc<Mutex<Lru<media_gpu::Texture>>> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(egui::Id::new("photo_gpu_cache"), || {
            Arc::new(Mutex::new(Lru::new(1024 * 1024 * 1024)))
        })
        .clone()
    })
}
pub fn default_ram_mb() -> usize {
    if video_cache::default_mb() >= 8192 {
        4096
    } else {
        512
    }
}
pub fn is_photo(source: &ImageSrc) -> bool {
    match source {
        ImageSrc::File(p) => is_image_ext(&ext_of_path(p)),
        ImageSrc::Entry { name, .. } => is_image_ext(&ext_of_name(name)),
    }
}
#[derive(Default)]
pub struct Client {
    context: Option<egui::Context>,
    token: Arc<AtomicU64>,
    reply: Arc<Mutex<Option<(u64, Decoded)>>>,
    pending: Option<Key>,
    neighbors: Vec<Key>,
    generation: u64,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Client {
    pub fn context(&self) -> Option<egui::Context> {
        self.context.clone()
    }
    pub fn cancel(&mut self) {
        self.generation = self.token.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        self.pending = None;
        self.neighbors.clear();
        *self.reply.lock().unwrap() = None;
    }
    pub fn disable_preload(&mut self) {
        self.neighbors.clear();
        engine()
            .queue
            .lock()
            .unwrap()
            .retain(|job| job.reply.is_some() || !Arc::ptr_eq(&job.token, &self.token));
    }
    pub fn configure(ctx: &egui::Context, ram_mb: usize, gpu_mb: usize) {
        engine().ram.lock().unwrap().configure(ram_mb * 1024 * 1024);
        gpu(ctx).lock().unwrap().configure(gpu_mb * 1024 * 1024);
    }
    pub fn request(
        &mut self,
        source: ImageSrc,
        neighbors: Vec<ImageSrc>,
        ctx: &egui::Context,
    ) -> Result<Option<media_gpu::Texture>> {
        self.cancel();
        self.context = Some(ctx.clone());
        let key = Key::new(&source)?;
        self.pending = Some(key.clone());
        let texture = gpu(ctx).lock().unwrap().get(&key);
        let texture = if texture.is_some() {
            texture
        } else {
            let cached = engine().ram.lock().unwrap().get(&key);
            match cached {
                Some(Ok(color)) => Some(upload(ctx, key.clone(), color)),
                Some(Err(e)) => return Err(anyhow::anyhow!(e)),
                None => None,
            }
        };
        let mut jobs = Vec::new();
        if texture.is_none() {
            jobs.push(Job {
                key: key.clone(),
                source,
                token: self.token.clone(),
                generation: self.generation,
                reply: Some(self.reply.clone()),
                ctx: ctx.clone(),
                epoch: engine().epoch.load(AtomicOrdering::Relaxed),
            });
        } else {
            self.pending = None;
        }
        for source in neighbors {
            if let Ok(neighbor) = Key::new(&source) {
                if neighbor == key || self.neighbors.contains(&neighbor) {
                    continue;
                }
                self.neighbors.push(neighbor.clone());
                if engine().ram.lock().unwrap().get(&neighbor).is_none() {
                    jobs.push(Job {
                        key: neighbor,
                        source,
                        token: self.token.clone(),
                        generation: self.generation,
                        reply: None,
                        ctx: ctx.clone(),
                        epoch: engine().epoch.load(AtomicOrdering::Relaxed),
                    });
                }
            }
        }
        let mut q = engine().queue.lock().unwrap();
        q.retain(|job| job.token.load(AtomicOrdering::Relaxed) == job.generation);
        for job in jobs {
            if job.reply.is_some() {
                q.push_front(job);
            } else if q.len() < 24 {
                q.push_back(job);
            }
        }
        drop(q);
        engine().wake.notify_all();
        ctx.request_repaint();
        Ok(texture)
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<std::result::Result<media_gpu::Texture, String>> {
        let result = self.reply.lock().unwrap().take();
        if let Some((generation, result)) = result {
            if generation == self.generation {
                if let Some(key) = self.pending.take() {
                    ctx.request_repaint();
                    return Some(result.map(|color| upload(ctx, key, color)));
                }
            }
        }
        if ctx.input(|i| {
            i.pointer.any_down()
                || i.raw_scroll_delta != Vec2::ZERO
                || i.events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
        }) {
            return None;
        }
        // At most one speculative GPU upload per idle frame. Do not evict textures for speculation.
        for key in self.neighbors.iter().take(3) {
            let cache = gpu(ctx);
            let mut cache = cache.lock().unwrap();
            if cache.get(key).is_some() {
                continue;
            }
            let Some(Ok(color)) = engine().ram.lock().unwrap().get(key) else {
                continue;
            };
            let bytes = color.gpu_bytes(ctx);
            if bytes > cache.budget.saturating_sub(cache.used) {
                continue;
            }
            let pass = ctx.cumulative_pass_nr();
            let already_uploaded = ctx.data_mut(|data| {
                let id = egui::Id::new("photo_upload_pass");
                if data.get_temp::<u64>(id) == Some(pass) {
                    true
                } else {
                    data.insert_temp(id, pass);
                    false
                }
            });
            if already_uploaded {
                ctx.request_repaint();
                break;
            }
            let texture = media_gpu::photo(ctx, &color);
            cache.put(key.clone(), texture, bytes);
            ctx.request_repaint();
            break;
        }
        None
    }
}
fn upload(ctx: &egui::Context, key: Key, color: Arc<media_gpu::Photo>) -> media_gpu::Texture {
    let bytes = color.gpu_bytes(ctx);
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("photo_upload_pass"), pass));
    let texture = media_gpu::photo(ctx, &color);
    gpu(ctx).lock().unwrap().put(key, texture.clone(), bytes);
    texture
}
pub fn stats(ctx: &egui::Context) -> (usize, usize, usize, usize) {
    let ram = engine().ram.lock().unwrap();
    let cache = gpu(ctx);
    let cache = cache.lock().unwrap();
    (
        ram.used / 1024 / 1024,
        ram.budget / 1024 / 1024,
        cache.used / 1024 / 1024,
        cache.budget / 1024 / 1024,
    )
}
pub fn clear(ctx: &egui::Context) {
    engine().epoch.fetch_add(1, AtomicOrdering::Relaxed);
    engine()
        .queue
        .lock()
        .unwrap()
        .retain(|job| job.reply.is_some());
    let mut ram = engine().ram.lock().unwrap();
    ram.items.clear();
    ram.used = 0;
    let cache = gpu(ctx);
    let mut cache = cache.lock().unwrap();
    cache.items.clear();
    cache.used = 0;
}
impl App {
    pub(super) fn request_photo(&mut self, i: usize, ctx: &egui::Context) {
        Client::configure(ctx, self.settings.photo_ram_mb, self.settings.photo_gpu_mb);
        let direction = if self.photo_last.is_some_and(|last| i < last) {
            -1
        } else {
            1
        };
        self.photo_last = Some(i);
        let mut neighbors = Vec::new();
        if self.settings.photo_preload && self.settings.photo_ram_mb > 0 {
            for delta in [direction, direction * 2, -direction, direction * 3] {
                let index = (i as isize + delta).rem_euclid(self.images.len() as isize) as usize;
                if index != i && is_photo(&self.images[index].src) {
                    neighbors.push(self.images[index].src.clone());
                }
            }
        }
        self.fit = true;
        self.pan = Vec2::ZERO;
        match self
            .photos
            .request(self.images[i].src.clone(), neighbors, ctx)
        {
            Ok(Some(texture)) => {
                self.tex_size = texture.size();
                self.texture = Some(texture);
                self.status = format!(
                    "{} / {} — {}",
                    i + 1,
                    self.images.len(),
                    self.images[i].label
                );
            }
            Ok(None) => {
                self.status = format!("Loading image: {}", self.images[i].label);
            }
            Err(error) => {
                self.status = format!("Image: {error}");
            }
        }
    }
    pub(super) fn poll_photo(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.photos.poll(ctx) {
            match result {
                Ok(texture) => {
                    self.tex_size = texture.size();
                    self.texture = Some(texture);
                    if let Some(i) = self.current {
                        self.status = format!(
                            "{} / {} — {}",
                            i + 1,
                            self.images.len(),
                            self.images[i].label
                        );
                    }
                }
                Err(error) => self.status = format!("Image: {error}"),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn wait_photo(&mut self, ctx: &egui::Context) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.photos.pending.is_some() && std::time::Instant::now() < until {
            self.poll_photo(ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(self.photos.pending.is_none(), "photo decode timed out");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, egui::Context, App) {
        let path = std::env::temp_dir().join(format!(
            "photo-pipeline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        for (i, w, h) in [(0, 24, 12), (1, 12, 24), (2, 36, 18)] {
            image::RgbaImage::from_pixel(w, h, image::Rgba([40, 80, 120, 255]))
                .save(path.join(format!("photo{i}.png")))
                .unwrap();
        }
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.select(Node::Dir(path.clone()), &ctx);
        (path, ctx, app)
    }
    #[test]
    fn newest_request_wins_cancelled_folder_and_gpu_hits_share_texture() {
        let (path, ctx, mut app) = fixture();
        app.goto(0, &ctx);
        app.goto(1, &ctx);
        app.wait_photo(&ctx);
        assert_eq!(app.tex_size, [12, 24]);
        let id = app.texture.as_ref().unwrap().id();
        app.goto(1, &ctx);
        assert_eq!(app.texture.as_ref().unwrap().id(), id);
        app.goto(2, &ctx);
        app.select(Node::Computer, &ctx);
        for _ in 0..10 {
            app.poll_photo(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.current.is_none() && app.texture.is_none());
        let mut other = App::new(None);
        other.select(Node::Dir(path.clone()), &ctx);
        other.goto(1, &ctx);
        assert_eq!(other.texture.as_ref().unwrap().id(), id);
        drop(app);
        drop(other);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn cache_limits_lru_zero_budget_and_file_replacement_are_respected() {
        let (path, ctx, mut app) = fixture();
        let source = ImageSrc::File(path.join("photo0.png"));
        let key = Key::new(&source).unwrap();
        let mut cache = Lru::new(10);
        let mut second = key.clone();
        second.entry = Some("other".into());
        cache.put(key.clone(), 1, 6);
        cache.put(second.clone(), 2, 6);
        assert_eq!(cache.used, 6);
        assert!(cache.get(&key).is_none());
        cache.put(key.clone(), 3, 20);
        assert_eq!(cache.get(&second), Some(2));
        cache.configure(0);
        assert_eq!((cache.used, cache.budget, cache.items.len()), (0, 0, 0));
        cache.put(key.clone(), 4, 6);
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.used, 0);
        app.goto(0, &ctx);
        app.wait_photo(&ctx);
        let first = app.texture.as_ref().unwrap().id();
        image::RgbaImage::new(63, 47)
            .save(path.join("photo0.png"))
            .unwrap();
        assert_ne!(key, Key::new(&source).unwrap());
        app.goto(0, &ctx);
        app.wait_photo(&ctx);
        assert_eq!(app.tex_size, [63, 47]);
        assert_ne!(first, app.texture.as_ref().unwrap().id());
        let settings =
            settings::Settings::parse("photo_preload=false\nphoto_ram_mb=1024\nphoto_gpu_mb=256");
        let saved = settings::Settings::parse(&settings.text());
        assert!(!saved.photo_preload);
        assert_eq!((saved.photo_ram_mb, saved.photo_gpu_mb), (1024, 256));
        app.settings.photo_ram_mb = 0;
        app.settings.photo_gpu_mb = 0;
        app.goto(0, &ctx);
        app.wait_photo(&ctx);
        assert!(app.texture.is_some());
        // The RAM LRU is process-wide: other parallel App tests legitimately
        // reconfigure it. Its zero-budget policy is checked on the isolated LRU
        // above; GPU caching belongs to this context and can be asserted here.
        let gpu_cache = gpu(&ctx);
        let gpu_cache = gpu_cache.lock().unwrap();
        assert_eq!((gpu_cache.used, gpu_cache.budget, gpu_cache.items.len()), (0, 0, 0));
        drop(gpu_cache);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    #[ignore = "manual release benchmark: full-resolution CPU decode and warm texture-cache hits; excludes renderer GPU upload"]
    fn benchmark_photo_preload() {
        let (path, ctx, mut app) = fixture();
        let pixels = image::RgbImage::from_fn(6000, 4000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        for i in 0..3 {
            pixels.save(path.join(format!("large{i}.jpg"))).unwrap();
        }
        app.select(Node::Dir(path.clone()), &ctx);
        let i = app
            .images
            .iter()
            .position(|image| image.label == "large0.jpg")
            .unwrap();
        let start = std::time::Instant::now();
        app.goto(i, &ctx);
        let enqueue = start.elapsed();
        app.wait_photo(&ctx);
        let cold = start.elapsed();
        let id = app.texture.as_ref().unwrap().id();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            app.goto(i, &ctx);
            assert_eq!(app.texture.as_ref().unwrap().id(), id);
        }
        let warm = start.elapsed() / 100;
        println!("6000x4000 photo: cold UI request {:?}; background decode + handoff {:?}; warm texture hit {:?}; limits {:?}",enqueue,cold,warm,stats(&ctx));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
