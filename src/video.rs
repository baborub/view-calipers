use std::{io::Read, path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::mpsc, time::Instant};
use anyhow::{anyhow, Context, Result};
use eframe::egui;

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 540;
const FPS: f64 = 24.0;
pub static GPU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn decoder_command() -> Command {
    let mut cmd = command("ffmpeg");
    cmd.args(["-v", "error", "-threads", "2"]);
    if GPU.load(std::sync::atomic::Ordering::Relaxed) { cmd.args(["-hwaccel", "auto"]); }
    cmd
}

// One pending request: dragging replaces queued positions instead of spawning a process per event.
pub struct Scrubber {
    pending: std::sync::Arc<(std::sync::Mutex<Option<f64>>, std::sync::Condvar)>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    frames: mpsc::Receiver<Result<egui::ColorImage>>,
    child: std::sync::Arc<std::sync::Mutex<Option<Child>>>,
}
impl Scrubber {
    pub fn new(path: PathBuf, ctx: egui::Context) -> Self {
        let pending = std::sync::Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, frames) = mpsc::channel();
        let child = std::sync::Arc::new(std::sync::Mutex::new(None::<Child>));
        let active = child.clone();
        let queue = pending.clone(); let stop = stopped.clone();
        std::thread::spawn(move || loop {
            let mut request = queue.0.lock().unwrap();
            while request.is_none() && !stop.load(std::sync::atomic::Ordering::Relaxed) {
                request = queue.1.wait(request).unwrap();
            }
            if stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
            let position: f64 = request.take().unwrap(); drop(request);
            let result = (|| -> Result<egui::ColorImage> {
                let mut guard = active.lock().unwrap();
                if stop.load(std::sync::atomic::Ordering::Relaxed) { return Err(anyhow!("Scrubbing cancelled")); }
                let mut process = decoder_command().args(["-ss", &position.to_string(), "-i"]).arg(&path)
                    .args(["-frames:v", "1", "-vf", "scale=960:540:force_original_aspect_ratio=decrease", "-f", "image2pipe", "-c:v", "png", "pipe:1"])
                    .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
                let mut stdout = process.stdout.take().unwrap();
                *guard = Some(process); drop(guard);
                let mut bytes = Vec::new();
                let read = stdout.read_to_end(&mut bytes);
                if let Some(mut process) = active.lock().unwrap().take() { let _ = process.wait(); }
                read?;
                let rgba = image::load_from_memory(&bytes)?.to_rgba8();
                Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
            })();
            if tx.send(result).is_err() { break; }
            ctx.request_repaint();
        });
        Self { pending, stopped, frames, child }
    }
    pub fn seek(&self, position: f64) { *self.pending.0.lock().unwrap() = Some(position); self.pending.1.notify_one(); }
    pub fn poll(&self) -> Option<Result<egui::ColorImage>> { self.frames.try_iter().last() }
}
impl Drop for Scrubber {
    fn drop(&mut self) {
        let _guard = self.pending.0.lock().unwrap();
        self.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
        self.pending.1.notify_one();
        if let Some(mut child) = self.child.lock().unwrap().take() { let _ = child.kill(); let _ = child.wait(); }
    }
}

pub fn command(name: &str) -> Command {
    let executable = format!("{name}.exe");
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_exe() {
        if let Some(parent) = path.parent() { candidates.push(parent.join("tools").join(&executable)); }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let packages = PathBuf::from(local).join("Microsoft/WinGet/Packages");
        if let Ok(entries) = std::fs::read_dir(packages) {
            for entry in entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("Gyan.FFmpeg_")) {
                if let Ok(builds) = std::fs::read_dir(entry.path()) {
                    for build in builds.flatten() { candidates.push(build.path().join("bin").join(&executable)); }
                }
            }
        }
    }
    let mut cmd = Command::new(candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name)));
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.stdin(Stdio::null());
    cmd
}

pub fn poster(path: &Path, seconds: f64, width: u32, height: u32) -> Result<Vec<u8>> {
    let output = decoder_command().args(["-ss", &seconds.to_string(), "-i"])
        .arg(path).args(["-frames:v", "1", "-vf", &format!("scale={width}:{height}:force_original_aspect_ratio=decrease"),
            "-f", "image2pipe", "-c:v", "png", "pipe:1"]).output().context("FFmpeg was not found for video playback")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(anyhow!("Could not read video frame: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

pub fn duration(path: &Path) -> Result<f64> {
    let output = command("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path).output().context("FFprobe was not found")?;
    let value: f64 = String::from_utf8_lossy(&output.stdout).trim().parse().context("Could not determine video duration")?;
    if !value.is_finite() || value <= 0.0 { return Err(anyhow!("Invalid video duration")); }
    Ok(value)
}

struct Session {
    decoder: Child,
    audio: Option<Child>,
    frames: mpsc::Receiver<Vec<u8>>,
    start: Option<Instant>,
    shown: usize,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.decoder.kill(); let _ = self.decoder.wait();
        if let Some(audio) = &mut self.audio { let _ = audio.kill(); let _ = audio.wait(); }
    }
}

pub struct Player {
    pub range: PlaybackRange,
    pub frame_size: [usize; 2],
    pub path: PathBuf,
    pub duration: f64,
    pub position: f64,
    pub muted: bool,
    session: Option<Session>,
    origin: f64,
}

#[derive(Clone, Debug)]
pub struct PlaybackRange {
    pub enabled: bool,
    pub repeat: bool,
    pub start: f64,
    pub end: f64,
}

impl PlaybackRange {
    pub fn new(duration: f64) -> Self { Self { enabled: false, repeat: false, start: 0.0, end: duration } }
    pub fn bounds(&self, duration: f64) -> (f64, f64) {
        if self.enabled { (self.start, self.end) } else { (0.0, duration) }
    }
    pub fn set_start(&mut self, time: f64, duration: f64) {
        self.start = time.clamp(0.0, (self.end - duration.min(1.0 / FPS)).max(0.0));
        self.enabled = true;
    }
    pub fn set_end(&mut self, time: f64, duration: f64) {
        self.end = time.clamp((self.start + duration.min(1.0 / FPS)).min(duration), duration);
        self.enabled = true;
    }
}

impl Player {
    pub fn new(path: PathBuf, frame_size: [usize; 2]) -> Result<Self> {
        if frame_size.contains(&0) { return Err(anyhow!("Invalid video frame size")); }
        let duration = duration(&path)?;
        Ok(Self { range: PlaybackRange::new(duration), frame_size, duration, path, position: 0.0, muted: false, session: None, origin: 0.0 })
    }
    pub fn playing(&self) -> bool { self.session.is_some() }
    pub fn pause(&mut self) { self.session = None; }
    pub fn play(&mut self, ctx: &egui::Context) -> Result<()> {
        self.pause();
        let (start, end) = self.range.bounds(self.duration);
        if self.position >= end || self.position < start { self.position = start; }
        self.origin = self.position;
        // The poster has already applied rotation metadata and aspect-preserving scaling.
        // Use that same display size for playback, without padding to a landscape canvas.
        let [width, height] = self.frame_size;
        let filter = format!("scale={width}:{height},setsar=1,fps={FPS}");
        let mut decoder = decoder_command().args(["-ss", &self.position.to_string(), "-i"])
            .arg(&self.path).args(["-t", &(end - self.position).to_string(), "-an", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let mut stdout = decoder.stdout.take().unwrap();
        let (tx, frames) = mpsc::sync_channel(3);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            loop {
                let mut bytes = vec![0; width * height * 4];
                if stdout.read_exact(&mut bytes).is_err() { break; }
                if tx.send(bytes).is_err() { break; }
                ctx.request_repaint();
            }
            ctx.request_repaint();
        });
        self.session = Some(Session { decoder, audio: None, frames, start: None, shown: 0 });
        Ok(())
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> Result<Option<egui::ColorImage>> {
        let (start, end) = self.range.bounds(self.duration);
        let Some(session) = self.session.as_mut() else { return Ok(None); };
        let target = session.start.map(|t| (t.elapsed().as_secs_f64() * FPS) as usize + 1).unwrap_or(1);
        let mut latest = None;
        let mut ended = false;
        while session.shown < target {
            match session.frames.try_recv() {
                Ok(bytes) => {
                    let position = self.origin + session.shown as f64 / FPS;
                    if position >= end { ended = true; break; }
                    if session.start.is_none() {
                        session.start = Some(Instant::now());
                        if !self.muted {
                            session.audio = command("ffplay").args(["-v", "error", "-nodisp", "-vn", "-autoexit", "-ss", &self.origin.to_string(), "-t", &(end - self.origin).to_string()])
                                .arg(&self.path).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
                        }
                    }
                    session.shown += 1;
                    self.position = (self.origin + (session.shown - 1) as f64 / FPS).min(self.duration);
                    latest = Some(egui::ColorImage::from_rgba_unmultiplied(self.frame_size, &bytes));
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => { ended = true; break; }
            }
        }
        if ended {
            self.pause();
            self.position = end;
            if self.range.repeat { self.position = start; self.play(ctx)?; }
        }
        Ok(latest)
    }
}
