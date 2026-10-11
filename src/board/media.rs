//! Per-object playback and read-only document cards; file probing stays off the UI thread.
use super::*;
use std::sync::{atomic::AtomicBool, Condvar};

enum Loaded {
    Video(video::Player, egui::ColorImage),
    Markdown(markdown::Markdown),
}
type Reply = (u64, u64, std::result::Result<Loaded, String>);
struct Job {
    id: u64,
    serial: u64,
    path: PathBuf,
    kind: MediaKind,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<Reply>,
    ctx: egui::Context,
}
struct Workers {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
}
fn workers() -> &'static Arc<Workers> {
    static WORKERS: OnceLock<Arc<Workers>> = OnceLock::new();
    WORKERS.get_or_init(|| {
        let pool = Arc::new(Workers {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        for _ in 0..2 {
            let pool = pool.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let mut q = pool.queue.lock().unwrap();
                    while q.is_empty() {
                        q = pool.wake.wait(q).unwrap();
                    }
                    q.pop_front().unwrap()
                };
                if job.cancel.load(AtomicOrdering::Relaxed) {
                    continue;
                }
                let result = load(&job.path, job.kind).map_err(|e| e.to_string());
                if !job.cancel.load(AtomicOrdering::Relaxed) {
                    let _ = job.tx.send((job.id, job.serial, result));
                    job.ctx.request_repaint();
                }
            });
        }
        pool
    })
}
fn load(path: &Path, kind: MediaKind) -> Result<Loaded> {
    match kind {
        MediaKind::Markdown => Ok(Loaded::Markdown(markdown::Markdown::load(path)?)),
        MediaKind::Video => {
            let bytes = video::poster(path, 0.0, 960, 540)?;
            let poster = image::load_from_memory(&bytes)?.to_rgb8();
            let size = [poster.width() as usize, poster.height() as usize];
            let mut player = video::Player::new(path.into(), size)?;
            player.muted = true;
            Ok(Loaded::Video(
                player,
                egui::ColorImage::from_rgb(size, poster.as_raw()),
            ))
        }
    }
}
pub(super) struct Video {
    pub player: video::Player,
    texture: Option<media_gpu::Texture>,
    scrubber: Option<video::Scrubber>,
    seeking: Option<f64>,
    resume: bool,
    pub error: Option<String>,
}
impl Video {
    fn pause(&mut self) {
        self.player.pause();
        self.scrubber = None;
        self.seeking = None;
        self.resume = false;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.scrubber.as_ref().and_then(video::Scrubber::poll) {
            match result {
                Ok(frame) => media_gpu::video(ctx, &mut self.texture, &frame),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        match self.player.poll(ctx) {
            Ok(Some(frame)) => {
                media_gpu::video(ctx, &mut self.texture, &frame);
                self.player.recycle(frame);
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.pause();
            }
            _ => {}
        }
        if let Some(delay) = self.player.next_repaint() {
            ctx.request_repaint_after(delay);
        }
    }
    fn play(&mut self, ctx: &egui::Context) {
        self.scrubber = None;
        self.seeking = None;
        self.resume = false;
        self.error = self.player.play(ctx).err().map(|e| e.to_string());
    }
    fn seek(&mut self, position: f64, commit: bool, ctx: &egui::Context) {
        let player = &mut self.player;
        let position = position.clamp(0.0, (player.duration - 1.0 / player.fps).max(0.0));
        if self.seeking.is_none() {
            self.resume = player.playing();
            player.pause();
        }
        player.position = position;
        self.seeking = Some(position);
        if commit && self.resume {
            self.play(ctx);
        } else {
            let scrubber = self.scrubber.get_or_insert_with(|| {
                video::Scrubber::new(
                    player.path.clone(),
                    player.frame_size,
                    player.fps,
                    ctx.clone(),
                )
            });
            if commit {
                scrubber.seek(position);
                self.seeking = None;
                self.resume = false;
            } else {
                scrubber.seek_preview(position);
            }
        }
    }
}
enum State {
    Video(Video),
    Markdown(markdown::Markdown),
}
struct Item {
    path: PathBuf,
    serial: u64,
    seen: u64,
    cancel: Arc<AtomicBool>,
    state: Option<State>,
    error: Option<String>,
}
impl Drop for Item {
    fn drop(&mut self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}
pub struct Media {
    items: HashMap<u64, Item>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    frame: u64,
    last_shown: u64,
    reader: Option<u64>,
}
impl Default for Media {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            items: HashMap::new(),
            tx,
            rx,
            frame: 0,
            last_shown: 0,
            reader: None,
        }
    }
}
impl Media {
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.last_shown = ctx.cumulative_pass_nr();
        self.frame += 1;
        for index in 0..4 {
            let Ok((id, serial, result)) = self.rx.try_recv() else {
                break;
            };
            if index == 3 {
                // Worker notifications may coalesce; keep draining a batch of ready cards.
                ctx.request_repaint();
            }
            let Some(item) = self.items.get_mut(&id).filter(|item| item.serial == serial) else {
                continue;
            };
            match result {
                Ok(Loaded::Markdown(doc)) => item.state = Some(State::Markdown(doc)),
                Ok(Loaded::Video(player, poster)) => {
                    item.state = Some(State::Video(Video {
                        player,
                        texture: Some(
                            ctx.load_texture("board-video", poster, TextureOptions::LINEAR)
                                .into(),
                        ),
                        scrubber: None,
                        seeking: None,
                        resume: false,
                        error: None,
                    }))
                }
                Err(error) => item.error = Some(error),
            }
        }
    }
    fn ensure(&mut self, id: u64, path: &Path, kind: MediaKind, ctx: &egui::Context) {
        if self.items.get(&id).is_some_and(|item| item.path != path) {
            self.items.remove(&id);
        }
        if !self.items.contains_key(&id) {
            if self.items.len() >= 32 {
                if let Some(old) = self
                    .items
                    .iter()
                    .filter(|(_, v)| v.seen + 1 < self.frame)
                    .min_by_key(|(_, v)| v.seen)
                    .map(|(k, _)| *k)
                {
                    self.items.remove(&old);
                } else {
                    return;
                }
            }
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let serial = SERIAL.fetch_add(1, AtomicOrdering::Relaxed);
            let cancel = Arc::new(AtomicBool::new(false));
            let pool = workers();
            let mut queue = pool.queue.lock().unwrap();
            queue.retain(|job| !job.cancel.load(AtomicOrdering::Relaxed));
            if queue.len() >= 64 {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            queue.push_back(Job {
                id,
                serial,
                path: path.into(),
                kind,
                cancel: cancel.clone(),
                tx: self.tx.clone(),
                ctx: ctx.clone(),
            });
            pool.wake.notify_one();
            self.items.insert(
                id,
                Item {
                    path: path.into(),
                    serial,
                    seen: self.frame,
                    cancel,
                    state: None,
                    error: None,
                },
            );
        }
        self.items.get_mut(&id).unwrap().seen = self.frame;
    }
    pub fn video_texture(
        &mut self,
        id: u64,
        path: &Path,
        ctx: &egui::Context,
    ) -> (Option<media_gpu::Texture>, Option<String>, bool) {
        self.ensure(id, path, MediaKind::Video, ctx);
        let Some(item) = self.items.get_mut(&id) else {
            return (None, Some("Zoom in to load more media".into()), false);
        };
        if let Some(State::Video(video)) = &mut item.state {
            video.poll(ctx);
            (
                video.texture.clone(),
                video.error.clone(),
                video.player.playing(),
            )
        } else {
            (None, item.error.clone(), false)
        }
    }
    pub fn paint_markdown(
        &mut self,
        painter: &egui::Painter,
        object: &Object,
        camera: Camera,
        area: Rect,
        path: &Path,
        ctx: &egui::Context,
    ) -> bool {
        self.ensure(object.id, path, MediaKind::Markdown, ctx);
        if let Some(State::Markdown(doc)) =
            self.items.get(&object.id).and_then(|i| i.state.as_ref())
        {
            doc.paint(painter, object, camera, area, path);
            true
        } else {
            false
        }
    }
    pub fn error(&self, id: u64) -> Option<&str> {
        self.items.get(&id).and_then(|i| i.error.as_deref())
    }
    pub fn reader_open(&self) -> bool {
        self.reader.is_some()
    }
    pub fn open_reader(&mut self, id: u64) {
        self.reader = Some(id);
    }
    pub fn toggle(&mut self, id: u64, ctx: &egui::Context) {
        let count = self
            .items
            .values()
            .filter(|i| matches!(&i.state,Some(State::Video(v)) if v.player.playing()))
            .count();
        if let Some(State::Video(video)) = self.items.get_mut(&id).and_then(|i| i.state.as_mut()) {
            if video.player.playing() {
                video.pause();
            } else if count < 8 {
                video.play(ctx);
            } else {
                video.error = Some("Pause another clip first (8 simultaneous players).".into());
            }
        }
    }
    pub fn controls(&mut self, ui: &mut egui::Ui, object: &Object, board: &Path) {
        let (source, kind) = match &object.content {
            Content::Video { source } => (source, MediaKind::Video),
            Content::Markdown { source } => (source, MediaKind::Markdown),
            _ => return,
        };
        let path = resolve(source, board);
        self.ensure(object.id, &path, kind, ui.ctx());
        let mut toggle = false;
        let mut reload = false;
        let mut read = false;
        let Some(item) = self.items.get_mut(&object.id) else {
            return;
        };
        ui.push_id(("board-media", object.id), |ui| {
            if let Some(State::Video(video)) = &mut item.state {
                let mut range_changed = false;
                ui.horizontal_wrapped(|ui| {
                    toggle = ui
                        .button(if video.player.playing() {
                            "Pause"
                        } else {
                            "Play"
                        })
                        .clicked();
                    if ui.checkbox(&mut video.player.muted, "Mute").changed()
                        && video.player.playing()
                    {
                        video.play(ui.ctx());
                    }
                    ui.checkbox(&mut video.player.range.repeat, "Loop");
                    range_changed |= ui
                        .checkbox(&mut video.player.range.enabled, "A–B")
                        .changed();
                    if ui.small_button("Reset range").clicked() {
                        let repeat = video.player.range.repeat;
                        video.player.range = video::PlaybackRange::new(video.player.duration);
                        video.player.range.repeat = repeat;
                        range_changed = true;
                    }
                    reload = ui.small_button("Reload").clicked();
                    ui.weak(file_name(&path));
                });
                let mut position = video.seeking.unwrap_or(video.player.position);
                let action = timeline::show(
                    ui,
                    &mut position,
                    video.player.duration,
                    &mut video.player.range,
                );
                if action.seeking {
                    video.seek(position, false, ui.ctx());
                }
                if let Some(position) = action.seek {
                    video.seek(position, true, ui.ctx());
                }
                if range_changed || action.range_committed {
                    let (start, end) = video.player.range.bounds(video.player.duration);
                    let position = video
                        .player
                        .position
                        .clamp(start, (end - 1.0 / video.player.fps).max(start));
                    video.seek(position, true, ui.ctx());
                }
                if let Some(error) = &video.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(file_name(&path));
                    if kind == MediaKind::Markdown {
                        read = ui
                            .add_enabled(item.state.is_some(), egui::Button::new("Read Markdown"))
                            .clicked();
                    }
                    reload = ui.small_button("Reload").clicked();
                    if item.state.is_none() && item.error.is_none() {
                        ui.weak("Loading…");
                    }
                });
                if let Some(error) = &item.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
            }
        });
        if toggle {
            self.toggle(object.id, ui.ctx());
        }
        if reload {
            self.items.remove(&object.id);
        }
        if read {
            self.reader = Some(object.id);
        }
    }
    pub fn reader_window(&mut self, ctx: &egui::Context, editor_id: u64) {
        let Some(id) = self.reader else {
            return;
        };
        let Some(item) = self.items.get_mut(&id) else {
            self.reader = None;
            return;
        };
        let mut open = true;
        egui::Window::new(format!("Markdown · {}", file_name(&item.path)))
            .id(egui::Id::new(("board-markdown", editor_id, id)))
            .open(&mut open)
            .default_size(Vec2::new(650.0, 540.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.weak("Read-only · source file stays unchanged");
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt(("reader", editor_id, id))
                    .show(ui, |ui| {
                        if let Some(State::Markdown(doc)) = &mut item.state {
                            doc.show(ui);
                        } else if let Some(error) = &item.error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        } else {
                            ui.label("Loading Markdown…");
                        }
                    });
            });
        if !open {
            self.reader = None;
        }
    }
    pub fn end(&mut self, doc: &Document, board: &Path) {
        self.items.retain(|id, item| {
            doc.objects.iter().any(|o| {
                o.id == *id
                    && match &o.content {
                        Content::Video { source } | Content::Markdown { source } => {
                            resolve(source, board) == item.path
                        }
                        _ => false,
                    }
            })
        });
        for item in self.items.values_mut().filter(|i| i.seen != self.frame) {
            if let Some(State::Video(v)) = &mut item.state {
                v.pause();
            }
        }
    }
    pub fn pause_when_hidden(&mut self, ctx: &egui::Context) {
        if self
            .items
            .values()
            .any(|i| matches!(&i.state,Some(State::Video(v)) if v.player.playing()))
        {
            if ctx.cumulative_pass_nr() > self.last_shown.saturating_add(1) {
                for item in self.items.values_mut() {
                    if let Some(State::Video(v)) = &mut item.state {
                        v.pause();
                    }
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(400));
            }
        }
    }
}

// Stream owned video/document copies; large clips never pass through a whole-file RAM buffer.
pub(super) fn copy_file(source: &Path, directory: &Path) -> Result<PathBuf> {
    use std::io::Write;
    let name = source
        .file_name()
        .context("Source filename missing")?
        .to_string_lossy();
    fileops::valid_name(&name)?;
    let stem = source.file_stem().unwrap().to_string_lossy();
    let ext = source.extension().unwrap_or_default().to_string_lossy();
    for index in 0..100_000 {
        let name = if index == 0 {
            name.to_string()
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let target = directory.join(name);
        let mut output = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        };
        let result = (|| -> Result<()> {
            let mut input = File::open(source)?;
            std::io::copy(&mut input, &mut output)?;
            output.flush()?;
            output.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            drop(output);
            let _ = std::fs::remove_file(&target);
            return Err(e);
        }
        return Ok(target);
    }
    anyhow::bail!("Could not choose a free filename")
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn clip(path: &Path, portrait: bool) {
        let size = if portrait { "90x160" } else { "160x90" };
        let out = video::command("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={size}:rate=20"),
                "-t",
                "1.2",
                "-c:v",
                "mpeg4",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn video_mut(bank: &mut Media, id: u64) -> &mut Video {
        let Some(State::Video(video)) = bank.items.get_mut(&id).and_then(|i| i.state.as_mut())
        else {
            panic!("Video not loaded")
        };
        video
    }
    #[test]
    fn independent_players_keep_portrait_geometry_seek_loop_and_stop_when_hidden() {
        let root = super::super::tests::fixture("canvas-playback");
        let a = root.join("portrait.mp4");
        let b = root.join("landscape.mp4");
        clip(&a, true);
        clip(&b, false);
        let ctx = egui::Context::default();
        let mut bank = Media::default();
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            bank.begin(&ctx);
            let first = bank.video_texture(1, &a, &ctx);
            let second = bank.video_texture(2, &b, &ctx);
            assert!(
                first.1.is_none() && second.1.is_none(),
                "{:?} {:?}",
                first.1,
                second.1
            );
            if first.0.is_some() && second.0.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "Background media load timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            video_mut(&mut bank, 1).player.frame_size[0]
                < video_mut(&mut bank, 1).player.frame_size[1]
        );
        assert!(
            video_mut(&mut bank, 2).player.frame_size[0]
                > video_mut(&mut bank, 2).player.frame_size[1]
        );
        bank.toggle(1, &ctx);
        bank.toggle(2, &ctx);
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            bank.video_texture(1, &a, &ctx);
            bank.video_texture(2, &b, &ctx);
            if video_mut(&mut bank, 1).player.position > 0.1
                && video_mut(&mut bank, 2).player.position > 0.1
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            video_mut(&mut bank, 1).texture.as_ref().unwrap().id(),
            video_mut(&mut bank, 2).texture.as_ref().unwrap().id()
        );
        bank.toggle(1, &ctx);
        assert!(!video_mut(&mut bank, 1).player.playing());
        assert!(video_mut(&mut bank, 2).player.playing());
        let v = video_mut(&mut bank, 1);
        v.seek(0.75, false, &ctx);
        v.seek(0.6, true, &ctx);
        assert!(!v.player.playing());
        assert!((v.player.position - 0.6).abs() < 0.001);
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(frame) = v.scrubber.as_ref().unwrap().poll() {
                assert_eq!(frame.unwrap().size, v.player.frame_size);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        v.player.range.enabled = true;
        v.player.range.start = 0.1;
        v.player.range.end = 0.35;
        v.player.range.repeat = true;
        v.play(&ctx);
        let mut last = 0.0;
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            v.poll(&ctx);
            let now = v.player.position;
            if last > 0.2 && now < last {
                break;
            }
            last = now;
            assert!(Instant::now() < deadline, "Clip did not repeat");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(v.player.playing());
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |_| {});
        }
        bank.pause_when_hidden(&ctx);
        assert!(
            !video_mut(&mut bank, 1).player.playing() && !video_mut(&mut bank, 2).player.playing()
        );
        bank.end(&Document::new("empty".into()), &root.join("board.vcboard"));
        assert!(bank.items.is_empty());
        drop(bank);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_media_can_be_reloaded_and_stale_replies_do_not_replace_relinked_files() {
        let root = super::super::tests::fixture("media-relink");
        let file = root.join("notes.md");
        std::fs::write(&file, "# Current document").unwrap();
        let ctx = egui::Context::default();
        let mut bank = Media::default();
        let missing = root.join("missing.md");
        bank.ensure(1, &missing, MediaKind::Markdown, &ctx);
        let deadline = Instant::now() + Duration::from_secs(3);
        while bank.error(1).is_none() {
            bank.begin(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let stale = bank.items[&1].serial;
        bank.ensure(1, &file, MediaKind::Markdown, &ctx);
        bank.tx
            .send((1, stale, Err("stale failure".into())))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            bank.begin(&ctx);
            if let Some(State::Markdown(doc)) = &bank.items[&1].state {
                assert_eq!(doc.text, "# Current document");
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(bank.error(1).is_none());
        let copy = copy_file(&file, &root).unwrap();
        assert_ne!(file, copy);
        assert_eq!(std::fs::read(file).unwrap(), std::fs::read(copy).unwrap());
        drop(bank);
        std::fs::remove_dir_all(root).unwrap();
    }
}
