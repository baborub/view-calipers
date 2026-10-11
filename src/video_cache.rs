//! One byte-bounded native-pixel cache for all panes and tabs. Sparse background frames
//! give immediate drag feedback; releasing the playhead always requests an exact frame.
use crate::video_frame::Format;
#[cfg(test)]
use crate::video_frame::Frame;
use eframe::egui;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const MIB: usize = 1024 * 1024;
pub static PRELOAD: AtomicBool = AtomicBool::new(false);
pub static CUDA_PRELOAD: AtomicBool = AtomicBool::new(true);
pub static FOREGROUND: AtomicUsize = AtomicUsize::new(0);

pub fn memory() -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe {
            GlobalMemoryStatusEx(&mut status).ok()?;
        }
        return Some((status.ullTotalPhys, status.ullAvailPhys));
    }
    #[cfg(not(windows))]
    {
        None
    }
}
pub fn default_mb() -> usize {
    match memory().map(|(total, _)| total / (1024 * 1024 * 1024)) {
        Some(gib) if gib >= 64 => 8192,
        Some(gib) if gib >= 32 => 2048,
        _ => 512,
    }
}

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub size: [usize; 2],
    pub format: Format,
    rate: u64,
    length: u64,
    modified: u128,
}
impl Source {
    pub fn new(path: &std::path::Path, size: [usize; 2], fps: f64) -> Option<Arc<Self>> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Arc::new(Self {
            path: std::fs::canonicalize(path).ok()?,
            size,
            format: Format::current(),
            rate: fps.to_bits(),
            length: metadata.len(),
            modified: metadata
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos(),
        }))
    }
    pub fn fps(&self) -> f64 {
        f64::from_bits(self.rate)
    }
}
#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    source: Arc<Source>,
    frame: i64,
}
struct Entry {
    pixels: Arc<Vec<u8>>,
    stamp: u64,
}
struct FrameCache {
    frames: HashMap<Key, Entry>,
    order: BTreeMap<u64, Key>,
    clock: u64,
    used: usize,
    budget: usize,
    epoch: u64,
    jobs: HashSet<Arc<Source>>,
    completed: HashSet<Arc<Source>>,
    retry: HashMap<Arc<Source>, Instant>,
    cuda_frames: usize,
}
impl FrameCache {
    fn new(budget: usize) -> Self {
        Self {
            frames: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            used: 0,
            budget,
            epoch: 0,
            jobs: HashSet::new(),
            completed: HashSet::new(),
            retry: HashMap::new(),
            cuda_frames: 0,
        }
    }
    fn get(&mut self, key: &Key) -> Option<Arc<Vec<u8>>> {
        let entry = self.frames.get_mut(key)?;
        self.order.remove(&entry.stamp);
        self.clock += 1;
        entry.stamp = self.clock;
        self.order.insert(entry.stamp, key.clone());
        Some(entry.pixels.clone())
    }
    fn evict(&mut self) {
        if let Some((_, key)) = self.order.pop_first() {
            if let Some(entry) = self.frames.remove(&key) {
                self.used -= entry.pixels.len();
            }
        }
    }
    fn put(&mut self, key: Key, pixels: Arc<Vec<u8>>, background: bool) -> bool {
        if self.frames.contains_key(&key) {
            return true;
        }
        if pixels.len() > self.budget {
            return false;
        }
        // Foreground scrubbing reserves a quarter of RAM capacity and may evict old frames.
        if background && self.used + pixels.len() > self.budget * 3 / 4 {
            return false;
        }
        while self.used + pixels.len() > self.budget {
            self.evict();
        }
        self.clock += 1;
        self.used += pixels.len();
        self.order.insert(self.clock, key.clone());
        self.frames.insert(
            key,
            Entry {
                pixels,
                stamp: self.clock,
            },
        );
        true
    }
}
fn cache() -> &'static Mutex<FrameCache> {
    static CACHE: OnceLock<Mutex<FrameCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FrameCache::new(default_mb() * MIB)))
}
pub fn configure(mb: usize, preload: bool, cuda: bool) {
    let changed = PRELOAD.swap(preload, Ordering::Relaxed) != preload;
    let changed = CUDA_PRELOAD.swap(cuda, Ordering::Relaxed) != cuda || changed;
    let mut cache = cache().lock().unwrap();
    if changed {
        cache.epoch += 1;
        cache.completed.clear();
        cache.retry.clear();
    }
    cache.budget = mb.min(32768) * MIB;
    while cache.used > cache.budget {
        cache.evict();
    }
}
pub fn clear() {
    let mut cache = cache().lock().unwrap();
    cache.frames.clear();
    cache.order.clear();
    cache.used = 0;
    cache.completed.clear();
    cache.retry.clear();
    cache.cuda_frames = 0;
    cache.epoch += 1;
}
pub fn stats() -> (usize, usize, usize, usize, usize) {
    let c = cache().lock().unwrap();
    (
        c.used / MIB,
        c.budget / MIB,
        c.frames.len(),
        c.jobs.len(),
        c.cuda_frames,
    )
}
pub fn get(source: &Arc<Source>, frame: i64) -> Option<Arc<Vec<u8>>> {
    cache().lock().unwrap().get(&Key {
        source: source.clone(),
        frame,
    })
}
pub fn nearest(source: &Arc<Source>, frame: i64) -> Option<Arc<Vec<u8>>> {
    let mut cache = cache().lock().unwrap();
    // At most a quarter-second displacement during dragging; never used for committed seeks.
    for distance in 0..=(source.fps() * 0.26).ceil() as i64 {
        for index in [frame - distance, frame + distance] {
            if let Some(pixels) = cache.get(&Key {
                source: source.clone(),
                frame: index,
            }) {
                return Some(pixels);
            }
        }
    }
    None
}
pub fn put(source: &Arc<Source>, frame: i64, pixels: Arc<Vec<u8>>) {
    let key = Key {
        source: source.clone(),
        frame,
    };
    let pressure = memory()
        .is_some_and(|(total, available)| available < (total / 10).max(2 * 1024 * 1024 * 1024));
    let mut cache = cache().lock().unwrap();
    if pressure {
        cache.evict();
        return;
    }
    if cache.budget > 0 && !cache.frames.contains_key(&key) {
        cache.put(key, pixels, false);
    }
}

pub struct Prefetcher {
    stopped: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}
impl Prefetcher {
    pub fn new(source: Arc<Source>, ctx: egui::Context) -> Option<Self> {
        if !PRELOAD.load(Ordering::Relaxed) {
            return None;
        }
        let epoch = {
            let mut c = cache().lock().unwrap();
            if c.budget == 0
                || c.used >= c.budget * 3 / 4
                || c.jobs.len() >= 2
                || c.completed.contains(&source)
                || c.retry
                    .get(&source)
                    .is_some_and(|until| *until > Instant::now())
                || !c.jobs.insert(source.clone())
            {
                return None;
            }
            c.epoch
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None::<Child>));
        let stop = stopped.clone();
        let active = child.clone();
        std::thread::spawn(move || {
            struct Job(Arc<Source>, Arc<AtomicBool>);
            impl Drop for Job {
                fn drop(&mut self) {
                    let mut c = cache().lock().unwrap();
                    c.jobs.remove(&self.0);
                    if !c.completed.contains(&self.0) {
                        c.retry
                            .insert(self.0.clone(), Instant::now() + Duration::from_secs(30));
                    }
                    self.1.store(true, Ordering::Relaxed);
                }
            }
            let _job = Job(source.clone(), stop.clone());
            let [width, height] = source.size;
            let Some(length) = source
                .format
                .length(source.size)
                .filter(|n| *n > 0 && *n <= crate::video::WIDTH * crate::video::HEIGHT * 3)
            else {
                return;
            };
            let fps = source.fps();
            let step = (fps / 2.0).round().max(1.0) as i64;
            let mut bytes = vec![0; length];
            // Rotation is performed by FFmpeg's CPU path; CUDA must not change its interpretation.
            let cuda = CUDA_PRELOAD.load(Ordering::Relaxed)
                && crate::video::GPU.load(Ordering::Relaxed)
                && no_rotation(&source.path);
            for use_cuda in if cuda { vec![true, false] } else { vec![false] } {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let mut command = crate::video::command("ffmpeg");
                command.args(["-v", "error", "-threads", "2"]);
                if use_cuda {
                    command.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
                }
                let scale = if use_cuda {
                    format!("scale_cuda={width}:{height}:format=yuv420p:interp_algo=bilinear,hwdownload,format=yuv420p,{}",source.format.scale(source.size))
                } else {
                    source.format.scale(source.size)
                };
                let filter = format!("fps={fps},select='not(mod(n,{step}))',{scale},setsar=1");
                let process = command
                    .args(["-i"])
                    .arg(&source.path)
                    .args([
                        "-map",
                        "0:v:0",
                        "-an",
                        "-vf",
                        &filter,
                        "-fps_mode",
                        "passthrough",
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        source.format.pixel_format(),
                        "pipe:1",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn();
                let Ok(mut process) = process else {
                    continue;
                };
                let mut stdout = process.stdout.take().unwrap();
                {
                    let mut guard = active.lock().unwrap();
                    if stop.load(Ordering::Relaxed) {
                        let _ = process.kill();
                        let _ = process.wait();
                        return;
                    }
                    *guard = Some(process);
                }
                let mut frame = 0_i64;
                let mut reached_eof = false;
                loop {
                    if stop.load(Ordering::Relaxed)
                        || !PRELOAD.load(Ordering::Relaxed)
                        || (use_cuda && !crate::video::GPU.load(Ordering::Relaxed))
                    {
                        break;
                    }
                    while FOREGROUND.load(Ordering::Relaxed) > 0 && !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    if let Some((total, available)) = memory() {
                        if available < (total / 10).max(2 * 1024 * 1024 * 1024) {
                            break;
                        }
                    }
                    if stdout.read_exact(&mut bytes).is_err() {
                        reached_eof = true;
                        break;
                    }
                    let accepted = {
                        let mut c = cache().lock().unwrap();
                        if c.epoch != epoch {
                            false
                        } else {
                            let inserted = c.put(
                                Key {
                                    source: source.clone(),
                                    frame,
                                },
                                Arc::new(bytes.clone()),
                                true,
                            );
                            if inserted && use_cuda {
                                c.cuda_frames += 1;
                            }
                            inserted
                        }
                    };
                    if !accepted {
                        break;
                    }
                    frame += step;
                    ctx.request_repaint();
                }
                let success = if let Some(mut process) = active.lock().unwrap().take() {
                    if reached_eof {
                        process.wait().is_ok_and(|s| s.success())
                    } else {
                        let _ = process.kill();
                        let _ = process.wait();
                        false
                    }
                } else {
                    false
                };
                if success {
                    cache().lock().unwrap().completed.insert(source.clone());
                    return;
                }
                if !use_cuda || frame > 0 || stop.load(Ordering::Relaxed) {
                    return;
                }
                // A missing CUDA filter, unsupported codec / GPU, or odd dimensions falls back.
            }
        });
        Some(Self { stopped, child })
    }
    pub fn running(&self) -> bool {
        !self.stopped.load(Ordering::Relaxed)
    }
}
impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn no_rotation(path: &std::path::Path) -> bool {
    let Ok(output) = crate::video::command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream_side_data=rotation:stream_tags=rotate",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .all(|(_, value)| {
                value
                    .parse::<f64>()
                    .is_ok_and(|rotation| rotation.rem_euclid(360.0).abs() < 0.01)
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_budget_lru_and_foreground_reserve_are_enforced() {
        let source = Arc::new(Source {
            path: "a.mp4".into(),
            size: [1, 1],
            format: Format::Rgb24,
            rate: 24_f64.to_bits(),
            length: 1,
            modified: 1,
        });
        let key = |frame| Key {
            source: source.clone(),
            frame,
        };
        let mut cache = FrameCache::new(12);
        assert!(cache.put(key(0), Arc::new(vec![1_u8; 3]), true));
        assert!(cache.put(key(1), Arc::new(vec![2_u8; 3]), true));
        assert!(cache.put(key(2), Arc::new(vec![3_u8; 3]), true));
        assert!(!cache.put(key(3), Arc::new(vec![4_u8; 3]), true));
        assert!(cache.put(key(3), Arc::new(vec![4_u8; 3]), false));
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.put(key(4), Arc::new(vec![5_u8; 3]), false));
        assert!(cache.get(&key(1)).is_none());
        assert_eq!(cache.used, 12);
        assert_eq!(cache.frames.len(), cache.order.len());
        assert!(!cache.put(key(5), Arc::new(vec![0_u8; 13]), false));
        let mut changed = (*source).clone();
        changed.modified = 2;
        assert!(cache
            .get(&Key {
                source: Arc::new(changed),
                frame: 0
            })
            .is_none());
    }

    #[test]
    #[ignore = "manual NVIDIA / FFmpeg RAM preload integration benchmark"]
    fn benchmark_ram_preload_cuda_and_random_scrubbing() {
        let directory =
            std::env::temp_dir().join(format!("view-calipers-ram-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let ctx = egui::Context::default();
        configure(512, true, true);
        crate::video::GPU.store(true, Ordering::Relaxed);
        for (name, codec) in [("gpu.mp4", "libx264"), ("fallback.mkv", "ffv1")] {
            clear();
            let path = directory.join(name);
            let mut encode = crate::video::command("ffmpeg");
            encode.args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1920x1080:rate=60",
                "-t",
                "4",
                "-c:v",
                codec,
                "-pix_fmt",
                "yuv420p",
            ]);
            if codec == "libx264" {
                encode.args(["-preset", "ultrafast", "-crf", "20"]);
            }
            let output = encode.arg(&path).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let source = Source::new(&path, [960, 540], 60.0).unwrap();
            let start = Instant::now();
            let preload = Prefetcher::new(source.clone(), ctx.clone()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            while preload.running() {
                assert!(Instant::now() < deadline, "Preload timeout");
                std::thread::sleep(Duration::from_millis(5));
            }
            let (used, _, count, _, cuda_frames) = stats();
            assert_eq!(count, 8, "Two sparse frames per second for four seconds");
            if codec == "libx264" {
                assert!(
                    cuda_frames > 0,
                    "Expected confirmed CUDA decoded/scaled output on RTX 4090"
                );
            } else {
                assert_eq!(cuda_frames, 0, "FFV1 should exercise the software fallback");
            }
            println!(
                "{codec}: preload {:.1} ms, {count} frames, {used} MB, {cuda_frames} CUDA frames",
                start.elapsed().as_secs_f64() * 1000.0
            );
            let scrubber = crate::video::Scrubber::new(path.clone(), [960, 540], 60.0, ctx.clone());
            let start = Instant::now();
            for position in [3.5, 0.5, 2.0, 0.0, 3.0, 1.5, 2.5, 1.0] {
                scrubber.seek(position);
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if let Some(frame) = scrubber.poll() {
                        assert_eq!(frame.unwrap().size, [960, 540]);
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            println!(
                "{codec}: random cached scrub {:.2} ms/seek",
                start.elapsed().as_secs_f64() * 125.0
            );
            // Match source time after normalizing to 60 fps; GPU bilinear resize may differ slightly.
            let cached = get(&source, 120).unwrap();
            let actual = Frame::new(source.size, cached, source.format)
                .color_image()
                .pixels
                .iter()
                .flat_map(|p| p.to_array()[..3].to_vec())
                .collect::<Vec<_>>();
            let reference = crate::video::command("ffmpeg")
                .args(["-v", "error", "-ss", "2", "-i"])
                .arg(&path)
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale=960:540:flags=bilinear",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "pipe:1",
                ])
                .output()
                .unwrap();
            assert!(reference.status.success());
            assert_eq!(actual.len(), reference.stdout.len());
            let error = actual
                .iter()
                .zip(&reference.stdout)
                .map(|(a, b)| a.abs_diff(*b) as u64)
                .sum::<u64>() as f64
                / actual.len() as f64;
            println!("{codec}: cached frame mean RGB difference at 2.0 s: {error:.3}");
            assert!(
                error < 4.0,
                "Cached frame must represent the requested source time"
            );
            drop(scrubber);
            drop(preload);
        }
        // Rotation metadata must bypass CUDA scaling and retain portrait geometry.
        clear();
        let rotated = directory.join("rotated.mp4");
        let output = crate::video::command("ffmpeg")
            .args(["-y", "-v", "error", "-display_rotation:v:0", "90", "-i"])
            .arg(directory.join("gpu.mp4"))
            .args(["-c", "copy"])
            .arg(&rotated)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!no_rotation(&rotated));
        let poster =
            image::load_from_memory(&crate::video::poster(&rotated, 0.5, 960, 540).unwrap())
                .unwrap()
                .to_rgb8();
        let size = [poster.width() as usize, poster.height() as usize];
        assert!(size[1] > size[0]);
        let source = Source::new(&rotated, size, 60.0).unwrap();
        let preload = Prefetcher::new(source.clone(), ctx.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while preload.running() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(stats().4, 0);
        let cached = get(&source, 30).unwrap();
        let pixels = Frame::new(source.size, cached, source.format)
            .color_image()
            .pixels
            .iter()
            .flat_map(|p| p.to_array()[..3].to_vec())
            .collect::<Vec<_>>();
        assert_eq!(pixels.len(), poster.as_raw().len());
        let error = pixels
            .iter()
            .zip(poster.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum::<u64>() as f64
            / pixels.len() as f64;
        assert!(
            error < 4.0,
            "Rotation/time must match the direct portrait poster"
        );
        drop(preload);
        // Closing a preview immediately cancels its job, including the pre-spawn case.
        clear();
        let cancelled = Prefetcher::new(source, ctx).unwrap();
        drop(cancelled);
        let deadline = Instant::now() + Duration::from_secs(3);
        while stats().3 > 0 {
            assert!(Instant::now() < deadline, "Cancelled preload job leaked");
            std::thread::sleep(Duration::from_millis(5));
        }
        configure(default_mb(), false, true);
        clear();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
