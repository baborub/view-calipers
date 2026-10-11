use crate::video_frame::{Format, Frame};
use anyhow::{anyhow, Context, Result};
use eframe::egui;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Instant,
};

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 540;
const FPS: f64 = 24.0; // fallback for files without usable rate metadata
pub static GPU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn decoder_command() -> Command {
    let mut cmd = command("ffmpeg");
    cmd.args(["-v", "error", "-threads", "2"]);
    if GPU.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.args(["-hwaccel", "auto"]);
    }
    cmd
}

// Decode neighboring scrub positions through one process. A backward jump outside the
// bounded cache or a large forward jump restarts the decoder with an input seek.
pub struct Scrubber {
    source: Option<std::sync::Arc<crate::video_cache::Source>>,
    pending: std::sync::Arc<(std::sync::Mutex<Option<(u64, f64)>>, std::sync::Condvar)>,
    requested: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    frames: std::sync::Arc<std::sync::Mutex<Option<(u64, Result<Frame>)>>>,
    child: std::sync::Arc<std::sync::Mutex<Option<Child>>>,
    #[cfg(test)]
    decoder_starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Scrubber {
    pub fn new(path: PathBuf, size: [usize; 2], fps: f64, ctx: egui::Context) -> Self {
        use std::sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, Condvar, Mutex,
        };
        let source = crate::video_cache::Source::new(&path, size, fps);
        let cache_source = source.clone();
        let pending = Arc::new((Mutex::new(None), Condvar::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(None));
        let output = frames.clone();
        let requested = Arc::new(AtomicU64::new(0));
        let newest = requested.clone();
        let child = Arc::new(Mutex::new(None::<Child>));
        let active = child.clone();
        let queue = pending.clone();
        let stop = stopped.clone();
        #[cfg(test)]
        let decoder_starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let starts = decoder_starts.clone();
        std::thread::spawn(move || {
            let fps = if fps.is_finite() && fps > 0.0 {
                fps.clamp(1.0, 120.0)
            } else {
                FPS
            };
            let format = cache_source
                .as_ref()
                .map_or_else(Format::current, |s| s.format);
            let Some(length) = format
                .length(size)
                .filter(|n| *n > 0 && *n <= WIDTH * HEIGHT * 3)
            else {
                return;
            };
            let mut bytes = vec![0; length];
            let mut cache = std::collections::VecDeque::<(i64, Frame)>::new();
            let mut stream = None::<std::process::ChildStdout>;
            let mut next = 0_i64;
            loop {
                let mut request = queue.0.lock().unwrap();
                while request.is_none() && !stop.load(Ordering::Relaxed) {
                    request = queue.1.wait(request).unwrap();
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let (id, position): (u64, f64) = request.take().unwrap();
                drop(request);
                let key = (position * fps).round() as i64;
                if let Some(index) = cache.iter().position(|(time, _)| *time == key) {
                    let item = cache.remove(index).unwrap();
                    *output.lock().unwrap() = Some((id, Ok(item.1.clone())));
                    cache.push_front(item);
                    ctx.request_repaint();
                    continue;
                }
                if let Some(pixels) = cache_source
                    .as_ref()
                    .and_then(|source| crate::video_cache::get(source, key))
                {
                    *output.lock().unwrap() = Some((id, Ok(Frame::new(size, pixels, format))));
                    ctx.request_repaint();
                    continue;
                }
                struct Foreground;
                impl Drop for Foreground {
                    fn drop(&mut self) {
                        crate::video_cache::FOREGROUND.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                crate::video_cache::FOREGROUND.fetch_add(1, Ordering::Relaxed);
                let _foreground = Foreground;
                let result = (|| -> Result<Option<Frame>> {
                    if stream.is_none() || key < next || key - next > fps.ceil() as i64 {
                        stream = None;
                        let mut guard = active.lock().unwrap();
                        if let Some(mut process) = guard.take() {
                            let _ = process.kill();
                            let _ = process.wait();
                        }
                        if stop.load(Ordering::Relaxed) {
                            return Ok(None);
                        }
                        let filter = format!("{},setsar=1,fps={fps}", format.scale(size));
                        let mut process = decoder_command()
                            .args(["-ss", &(key as f64 / fps).to_string(), "-i"])
                            .arg(&path)
                            .args([
                                "-an",
                                "-vf",
                                &filter,
                                "-f",
                                "rawvideo",
                                "-pix_fmt",
                                format.pixel_format(),
                                "pipe:1",
                            ])
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .spawn()?;
                        stream = process.stdout.take();
                        *guard = Some(process);
                        next = key;
                        #[cfg(test)]
                        starts.fetch_add(1, Ordering::Relaxed);
                    }
                    while next <= key {
                        stream
                            .as_mut()
                            .unwrap()
                            .read_exact(&mut bytes)
                            .context("Could not decode scrub frame")?;
                        let index = next;
                        next += 1;
                        let pixels = std::sync::Arc::new(bytes.clone());
                        if let Some(source) = &cache_source {
                            crate::video_cache::put(source, index, pixels.clone());
                        }
                        if index >= key - 7 {
                            let image = Frame::new(size, pixels, format);
                            cache.push_front((index, image.clone()));
                            if cache.len() > 8 {
                                cache.pop_back();
                            }
                            if index == key {
                                return Ok(Some(image));
                            }
                        }
                        if newest.load(Ordering::Relaxed) != id || stop.load(Ordering::Relaxed) {
                            return Ok(None);
                        }
                    }
                    Ok(None)
                })();
                if result.is_err() {
                    stream = None;
                    if let Some(mut process) = active.lock().unwrap().take() {
                        let _ = process.kill();
                        let _ = process.wait();
                    }
                }
                if newest.load(Ordering::Relaxed) != id {
                    continue;
                }
                let result = match result {
                    Ok(Some(image)) => Ok(image),
                    Ok(None) => continue,
                    Err(error) => Err(error),
                };
                *output.lock().unwrap() = Some((id, result));
                ctx.request_repaint();
            }
        });
        Self {
            source,
            pending,
            requested,
            stopped,
            frames,
            child,
            #[cfg(test)]
            decoder_starts,
        }
    }
    pub fn seek_preview(&self, position: f64) {
        self.request(position, true);
    }
    pub fn seek(&self, position: f64) {
        self.request(position, false);
    }
    fn request(&self, position: f64, approximate: bool) {
        if !position.is_finite() {
            return;
        }
        let mut pending = self.pending.0.lock().unwrap();
        let id = self
            .requested
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if approximate {
            if let Some((source, pixels)) = self.source.as_ref().and_then(|source| {
                crate::video_cache::nearest(
                    source,
                    (position.max(0.0) * source.fps()).round() as i64,
                )
                .map(|pixels| (source, pixels))
            }) {
                *self.frames.lock().unwrap() =
                    Some((id, Ok(Frame::new(source.size, pixels, source.format))));
            }
        }
        *pending = Some((id, position.max(0.0)));
        self.pending.1.notify_one();
    }
    pub fn poll(&self) -> Option<Result<Frame>> {
        self.frames.lock().unwrap().take().and_then(|(id, frame)| {
            (id == self.requested.load(std::sync::atomic::Ordering::Relaxed)).then_some(frame)
        })
    }
}
impl Drop for Scrubber {
    fn drop(&mut self) {
        let _guard = self.pending.0.lock().unwrap();
        self.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.pending.1.notify_one();
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn command(name: &str) -> Command {
    let executable = format!("{name}.exe");
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_exe() {
        if let Some(parent) = path.parent() {
            candidates.push(parent.join("tools").join(&executable));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let packages = PathBuf::from(local).join("Microsoft/WinGet/Packages");
        if let Ok(entries) = std::fs::read_dir(packages) {
            for entry in entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Gyan.FFmpeg_"))
            {
                if let Ok(builds) = std::fs::read_dir(entry.path()) {
                    for build in builds.flatten() {
                        candidates.push(build.path().join("bin").join(&executable));
                    }
                }
            }
        }
    }
    let mut cmd = Command::new(
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from(name)),
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.stdin(Stdio::null());
    cmd
}

pub fn poster(path: &Path, seconds: f64, width: u32, height: u32) -> Result<Vec<u8>> {
    let output = decoder_command()
        .args(["-ss", &seconds.to_string(), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={width}:{height}:force_original_aspect_ratio=decrease"),
            "-f",
            "image2pipe",
            "-c:v",
            "ppm",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .context("FFmpeg was not found for video playback")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(anyhow!(
            "Could not read video frame: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

fn media_info(path: &Path) -> Result<(f64, f64)> {
    let output = command("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=avg_frame_rate",
            "-select_streams",
            "v:0",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .context("FFprobe was not found")?;
    if !output.status.success() {
        return Err(anyhow!("Could not read video metadata"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value: f64 = text
        .lines()
        .find_map(|line| line.strip_prefix("duration="))
        .context("Missing video duration")?
        .parse()
        .context("Could not determine video duration")?;
    let fps = text
        .lines()
        .find_map(|line| line.strip_prefix("avg_frame_rate="))
        .and_then(parse_frame_rate)
        .unwrap_or(FPS);
    if !value.is_finite() || value <= 0.0 {
        return Err(anyhow!("Invalid video duration"));
    }
    Ok((value, fps))
}

fn parse_frame_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let rate = numerator.parse::<f64>().ok()? / denominator.parse::<f64>().ok()?;
    (rate.is_finite() && rate > 0.0).then(|| rate.clamp(1.0, 120.0))
}

struct Session {
    decoder: Child,
    audio: Option<Child>,
    frames: mpsc::Receiver<Vec<u8>>,
    recycle: mpsc::SyncSender<Vec<u8>>,
    start: Option<Instant>,
    shown: usize,
    wake: std::sync::Arc<std::sync::atomic::AtomicBool>,
    waiting: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.decoder.kill();
        let _ = self.decoder.wait();
        if let Some(audio) = &mut self.audio {
            let _ = audio.kill();
            let _ = audio.wait();
        }
    }
}

pub struct Player {
    cache_source: Option<std::sync::Arc<crate::video_cache::Source>>,
    prefetch: Option<crate::video_cache::Prefetcher>,
    pub range: PlaybackRange,
    pub frame_size: [usize; 2],
    pub path: PathBuf,
    pub duration: f64,
    pub fps: f64,
    pub position: f64,
    pub muted: bool,
    session: Option<Session>,
    origin: f64,
    format: Format,
}

#[derive(Clone, Debug)]
pub struct PlaybackRange {
    pub enabled: bool,
    pub repeat: bool,
    pub start: f64,
    pub end: f64,
}

impl PlaybackRange {
    pub fn new(duration: f64) -> Self {
        Self {
            enabled: false,
            repeat: false,
            start: 0.0,
            end: duration,
        }
    }
    pub fn bounds(&self, duration: f64) -> (f64, f64) {
        if self.enabled {
            (self.start, self.end)
        } else {
            (0.0, duration)
        }
    }
    pub fn set_start(&mut self, time: f64, duration: f64) {
        self.start = time.clamp(0.0, (self.end - duration.min(1.0 / FPS)).max(0.0));
        self.enabled = true;
    }
    pub fn set_end(&mut self, time: f64, duration: f64) {
        self.end = time.clamp(
            (self.start + duration.min(1.0 / FPS)).min(duration),
            duration,
        );
        self.enabled = true;
    }
}

impl Player {
    pub fn new(path: PathBuf, frame_size: [usize; 2]) -> Result<Self> {
        if frame_size.contains(&0) {
            return Err(anyhow!("Invalid video frame size"));
        }
        let (duration, fps) = media_info(&path)?;
        Ok(Self {
            cache_source: crate::video_cache::Source::new(&path, frame_size, fps),
            prefetch: None,
            range: PlaybackRange::new(duration),
            frame_size,
            duration,
            fps,
            path,
            position: 0.0,
            muted: false,
            session: None,
            origin: 0.0,
            format: Format::current(),
        })
    }
    pub fn preload(&mut self, ctx: &egui::Context) {
        if !crate::video_cache::PRELOAD.load(std::sync::atomic::Ordering::Relaxed) {
            self.prefetch = None;
            return;
        }
        if self.prefetch.as_ref().is_some_and(|p| !p.running()) {
            self.prefetch = None;
        }
        if self.prefetch.is_none() {
            if let Some(source) = &self.cache_source {
                self.prefetch = crate::video_cache::Prefetcher::new(source.clone(), ctx.clone());
            }
        }
    }
    pub fn recycle(&self, frame: Frame) {
        if let (Some(session), Ok(bytes)) =
            (&self.session, std::sync::Arc::try_unwrap(frame.pixels))
        {
            let _ = session.recycle.try_send(bytes);
        }
    }
    pub fn next_repaint(&self) -> Option<std::time::Duration> {
        let session = self.session.as_ref()?;
        if session.waiting {
            return None;
        }
        let start = session.start?; // First decoded frame wakes the UI itself.
        let due = session.shown as f64 / self.fps;
        Some(std::time::Duration::from_secs_f64(
            (due - start.elapsed().as_secs_f64()).max(0.001),
        ))
    }
    pub fn playing(&self) -> bool {
        self.session.is_some()
    }
    pub fn pause(&mut self) {
        self.session = None;
    }
    pub fn play(&mut self, ctx: &egui::Context) -> Result<()> {
        self.pause();
        let (start, end) = self.range.bounds(self.duration);
        if self.position >= end || self.position < start {
            self.position = start;
        }
        self.origin = self.position;
        // The poster has already applied rotation metadata and aspect-preserving scaling.
        // Use that same display size for playback, without padding to a landscape canvas.
        let [width, height] = self.frame_size;
        let filter = format!(
            "{},setsar=1,fps={}",
            self.format.scale(self.frame_size),
            self.fps
        );
        let mut decoder = decoder_command()
            .args(["-ss", &self.position.to_string(), "-i"])
            .arg(&self.path)
            .args([
                "-t",
                &(end - self.position).to_string(),
                "-an",
                "-vf",
                &filter,
                "-f",
                "rawvideo",
                "-pix_fmt",
                self.format.pixel_format(),
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = decoder.stdout.take().unwrap();
        let length = self
            .format
            .length([width, height])
            .ok_or_else(|| anyhow!("Invalid video layout"))?;
        let (tx, frames) = mpsc::sync_channel(3);
        let (recycle, buffers) = mpsc::sync_channel(4);
        let ctx = ctx.clone();
        let wake = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let signal = wake.clone();
        std::thread::spawn(move || {
            loop {
                let mut bytes = buffers.try_recv().unwrap_or_else(|_| vec![0; length]);
                if stdout.read_exact(&mut bytes).is_err() {
                    break;
                }
                if tx.send(bytes).is_err() {
                    break;
                }
                if signal.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    ctx.request_repaint();
                }
            }
            ctx.request_repaint();
        });
        self.session = Some(Session {
            decoder,
            audio: None,
            frames,
            recycle,
            start: None,
            shown: 0,
            wake,
            waiting: true,
        });
        Ok(())
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> Result<Option<Frame>> {
        let (start, end) = self.range.bounds(self.duration);
        let Some(session) = self.session.as_mut() else {
            return Ok(None);
        };
        let target = session
            .start
            .map(|t| (t.elapsed().as_secs_f64() * self.fps) as usize + 1)
            .unwrap_or(1);
        let mut latest = None;
        let mut ended = false;
        session.waiting = false;
        while session.shown < target {
            // Arm before checking the channel so a producer cannot race a sleeping UI.
            session
                .wake
                .store(true, std::sync::atomic::Ordering::Release);
            match session.frames.try_recv() {
                Ok(bytes) => {
                    session
                        .wake
                        .store(false, std::sync::atomic::Ordering::Release);
                    let position = self.origin + session.shown as f64 / self.fps;
                    if position >= end {
                        ended = true;
                        break;
                    }
                    if session.start.is_none() {
                        session.start = Some(Instant::now());
                        if !self.muted {
                            session.audio = command("ffplay")
                                .args([
                                    "-v",
                                    "error",
                                    "-nodisp",
                                    "-vn",
                                    "-autoexit",
                                    "-ss",
                                    &self.origin.to_string(),
                                    "-t",
                                    &(end - self.origin).to_string(),
                                ])
                                .arg(&self.path)
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                                .ok();
                        }
                    }
                    session.shown += 1;
                    self.position =
                        (self.origin + (session.shown - 1) as f64 / self.fps).min(self.duration);
                    if let Some(previous) = latest.replace(bytes) {
                        let _ = session.recycle.try_send(previous);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    session.waiting = true;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        // The displayed frame keeps its decoder layout; the renderer converts YUV on the GPU.
        let latest = latest
            .map(|bytes| Frame::new(self.frame_size, std::sync::Arc::new(bytes), self.format));
        if ended {
            self.pause();
            self.position = end;
            if self.range.repeat {
                self.position = start;
                self.play(ctx)?;
            }
        }
        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_frame_rates_and_invalid_metadata() {
        assert_eq!(parse_frame_rate("60/1"), Some(60.0));
        assert!((parse_frame_rate("30000/1001").unwrap() - 29.97003).abs() < 0.00001);
        assert_eq!(parse_frame_rate("240/1"), Some(120.0));
        for value in ["0/0", "0/1", "1/0", "N/A", "-24/1", "NaN/1"] {
            assert_eq!(parse_frame_rate(value), None);
        }
    }

    #[test]
    #[ignore = "manual FFmpeg release benchmark"]
    fn benchmark_video_frame_transport() {
        let path =
            std::env::temp_dir().join(format!("view-calipers-bench-{}.mp4", std::process::id()));
        let output = command("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1920x1080:rate=60",
                "-t",
                "2",
                "-c:v",
                "mpeg4",
                "-q:v",
                "3",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for codec in ["png", "ppm"] {
            let start = Instant::now();
            for _ in 0..5 {
                let output = decoder_command()
                    .args(["-ss", "0.5", "-i"])
                    .arg(&path)
                    .args([
                        "-frames:v",
                        "1",
                        "-vf",
                        "scale=960:540:force_original_aspect_ratio=decrease",
                        "-f",
                        "image2pipe",
                        "-c:v",
                        codec,
                        "pipe:1",
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let frame = image::load_from_memory(&output.stdout).unwrap();
                assert_eq!((frame.width(), frame.height()), (960, 540));
                std::hint::black_box(frame);
            }
            println!(
                "1080p video {codec} extract + decode: {:.1} ms/frame",
                start.elapsed().as_secs_f64() * 200.0
            );
        }
        let ctx = egui::Context::default();
        let scrubber = Scrubber::new(path.clone(), [960, 540], 60.0, ctx);
        let start = Instant::now();
        for index in 0..10 {
            scrubber.seek(index as f64 / 10.0);
            wait_scrub(&scrubber);
        }
        println!(
            "1080p sequential scrubbing: {:.1} ms/seek (including first decoder startup)",
            start.elapsed().as_secs_f64() * 100.0
        );
        assert_eq!(
            scrubber
                .decoder_starts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        scrubber.seek(0.85); // recent backward position is cached
        wait_scrub(&scrubber);
        assert_eq!(
            scrubber
                .decoder_starts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        scrubber.seek(0.0); // backward position remains in the shared RAM cache
        wait_scrub(&scrubber);
        assert_eq!(
            scrubber
                .decoder_starts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        scrubber.seek(0.5);
        let actual = wait_scrub(&scrubber);
        let reference = image::load_from_memory(&poster(&path, 0.5, 960, 540).unwrap())
            .unwrap()
            .to_rgb8();
        let reference = egui::ColorImage::from_rgb([960, 540], reference.as_raw());
        let error = actual
            .pixels
            .iter()
            .zip(&reference.pixels)
            .map(|(a, b)| {
                a.to_array()[..3]
                    .iter()
                    .zip(&b.to_array()[..3])
                    .map(|(a, b)| a.abs_diff(*b) as u64)
                    .sum::<u64>()
            })
            .sum::<u64>() as f64
            / (actual.pixels.len() * 3) as f64;
        assert!(
            error < 5.0,
            "Persistent seek source frame difference {error}"
        );
        drop(scrubber);
        std::fs::remove_file(path).unwrap();
    }

    fn wait_scrub(scrubber: &Scrubber) -> egui::ColorImage {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = scrubber.poll() {
                let image = result.unwrap();
                assert_eq!(image.size, [960, 540]);
                return image.color_image();
            }
            assert!(Instant::now() < deadline, "Scrub decoder timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
