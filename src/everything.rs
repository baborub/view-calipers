use super::*;
use std::sync::{Condvar, OnceLock};
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub folder: bool,
    pub bytes: Option<u64>,
    pub created: String,
    pub modified: String,
}
pub struct Results {
    pub hits: Vec<Hit>,
    pub total: usize,
    pub elapsed: std::time::Duration,
}
type Reply = std::result::Result<Results, String>;
#[derive(Default)]
pub struct Client {
    token: Arc<AtomicU64>,
    reply: Arc<Mutex<Option<(u64, Reply)>>>,
    pub busy: bool,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Client {
    pub fn cancel(&mut self) {
        self.token.fetch_add(1, AtomicOrdering::Relaxed);
        self.busy = false;
        *self.reply.lock().unwrap() = None;
    }
    pub fn search_sorted(&mut self, query:String,offset:usize,sort:u32,ctx:&egui::Context){
        self.cancel();
        self.busy = true;
        let generation = self.token.load(AtomicOrdering::Relaxed);
        let worker = worker();
        let mut q = worker.queue.lock().unwrap();
        q.retain(|j| j.token.load(AtomicOrdering::Relaxed) == j.generation);
        q.push_back(Job {
            query,
            offset,
            sort,
            token: self.token.clone(),
            generation,
            reply: self.reply.clone(),
            ctx: ctx.clone(),
        });
        drop(q);
        worker.wake.notify_one();
    }
    pub fn poll(&mut self) -> Option<Reply> {
        let result = self.reply.lock().unwrap().take();
        if let Some((g, r)) = result {
            if g == self.token.load(AtomicOrdering::Relaxed) {
                self.busy = false;
                return Some(r);
            }
        }
        None
    }
}
struct Job {
    sort:u32,
    query: String,
    offset: usize,
    token: Arc<AtomicU64>,
    generation: u64,
    reply: Arc<Mutex<Option<(u64, Reply)>>>,
    ctx: egui::Context,
}
struct Worker {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
}
fn worker() -> &'static Arc<Worker> {
    static WORKER: OnceLock<Arc<Worker>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let worker = Arc::new(Worker {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        let w = worker.clone();
        std::thread::spawn(move || loop {
            let job = {
                let mut q = w.queue.lock().unwrap();
                while q.is_empty() {
                    q = w.wake.wait(q).unwrap();
                }
                q.pop_front().unwrap()
            };
            if job.token.load(AtomicOrdering::Relaxed) != job.generation {
                continue;
            }
            let result = query_sorted(&job.query, job.offset,job.sort);
            let mut reply = job.reply.lock().unwrap();
            if job.token.load(AtomicOrdering::Relaxed) == job.generation {
                *reply = Some((job.generation, result));
            }
            drop(reply);
            job.ctx.request_repaint();
        });
        worker
    })
}
#[cfg(all(windows,test))]
fn query(query:&str,offset:usize)->Reply { query_sorted(query,offset,1) }
#[cfg(windows)]
fn query_sorted(query: &str, offset: usize,sort:u32) -> Reply {
    static SDK: OnceLock<std::result::Result<Sdk, String>> = OnceLock::new();
    let sdk = SDK
        .get_or_init(|| Sdk::load().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)?;
    let start = std::time::Instant::now();
    let search: Vec<u16> = query.encode_utf16().chain(Some(0)).collect();
    unsafe {
        (sdk.reset)();
        (sdk.set_search)(search.as_ptr());
        (sdk.set_max)(200);
        (sdk.set_offset)(offset.min(u32::MAX as usize) as u32);
        (sdk.set_sort)(sort);
        (sdk.set_flags)(1 | 2 | 4 | 16 | 32 | 64);
        if (sdk.query)(1) == 0 {
            return Err(match (sdk.error)() {
                2 => "Everything is not running. Use Start Everything, then retry.".into(),
                code => format!(
                    "Everything query failed (SDK error {code}). Check that its index is ready."
                ),
            });
        }
        let total = (sdk.total)() as usize;
        let count = (sdk.count)().min(200);
        let mut hits = Vec::new();
        let mut path = vec![0u16; 32768];
        for i in 0..count {
            let n = (sdk.path)(i, path.as_mut_ptr(), path.len() as u32) as usize;
            if n == 0 || n >= path.len() {
                continue;
            }
            let path = PathBuf::from(std::ffi::OsString::from_wide(&path[..n]));
            let folder = (sdk.folder)(i) != 0;
            let mut bytes = 0i64;
            let bytes = if !folder && (sdk.size)(i, &mut bytes) != 0 && bytes >= 0 {
                Some(bytes as u64)
            } else {
                None
            };
            let mut created = 0u64;
            let mut modified = 0u64;
            (sdk.created)(i, &mut created);
            (sdk.modified)(i, &mut modified);
            hits.push(Hit {
                path,
                folder,
                bytes,
                created: format_time(created),
                modified: format_time(modified),
            });
        }
        Ok(Results {
            hits,
            total,
            elapsed: start.elapsed(),
        })
    }
}
#[cfg(not(windows))]
fn query(_: &str, _: usize) -> Reply {query_sorted("",0,1)}
#[cfg(not(windows))]
fn query_sorted(_: &str, _: usize,_:u32) -> Reply {
    Err("Everything search is available on Windows".into())
}
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
struct Sdk {
    reset: unsafe extern "system" fn(),
    set_search: unsafe extern "system" fn(*const u16),
    set_max: unsafe extern "system" fn(u32),
    set_offset: unsafe extern "system" fn(u32),
    set_sort: unsafe extern "system" fn(u32),
    set_flags: unsafe extern "system" fn(u32),
    query: unsafe extern "system" fn(i32) -> i32,
    error: unsafe extern "system" fn() -> u32,
    total: unsafe extern "system" fn() -> u32,
    count: unsafe extern "system" fn() -> u32,
    path: unsafe extern "system" fn(u32, *mut u16, u32) -> u32,
    folder: unsafe extern "system" fn(u32) -> i32,
    size: unsafe extern "system" fn(u32, *mut i64) -> i32,
    created: unsafe extern "system" fn(u32, *mut u64) -> i32,
    modified: unsafe extern "system" fn(u32, *mut u64) -> i32,
}
#[cfg(windows)]
impl Sdk {
    fn load() -> Result<Self> {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryExW(path: *const u16, file: usize, flags: u32) -> usize;
            fn GetProcAddress(module: usize, name: *const u8) -> *mut std::ffi::c_void;
        }
        static BYTES: &[u8] = include_bytes!("../vendor/everything/Everything64.dll");
        let hash = BYTES.iter().fold(14695981039346656037u64, |hash, b| {
            (hash ^ *b as u64).wrapping_mul(1099511628211)
        });
        let dir = settings::Settings::path().parent().unwrap().join("runtime");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("Everything64-{hash:016x}.dll"));
        if !path.exists() {
            let temp = path.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temp, BYTES)?;
            std::fs::rename(temp, &path)?;
        }
        anyhow::ensure!(
            std::fs::read(&path)? == BYTES,
            "Bundled Everything SDK checksum mismatch"
        );
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let module = LoadLibraryExW(path.as_ptr(), 0, 0x100 | 0x800);
            anyhow::ensure!(module != 0, "Could not load Everything SDK");
            macro_rules! f {
                ($name:literal,$ty:ty) => {{
                    let ptr = GetProcAddress(module, concat!($name, "\0").as_ptr());
                    anyhow::ensure!(!ptr.is_null(), "SDK export missing: {}", $name);
                    std::mem::transmute::<*mut std::ffi::c_void, $ty>(ptr)
                }};
            }
            Ok(Self {
                reset: f!("Everything_Reset", unsafe extern "system" fn()),
                set_search: f!(
                    "Everything_SetSearchW",
                    unsafe extern "system" fn(*const u16)
                ),
                set_max: f!("Everything_SetMax", unsafe extern "system" fn(u32)),
                set_offset: f!("Everything_SetOffset", unsafe extern "system" fn(u32)),
                set_sort: f!("Everything_SetSort", unsafe extern "system" fn(u32)),
                set_flags: f!("Everything_SetRequestFlags", unsafe extern "system" fn(u32)),
                query: f!("Everything_QueryW", unsafe extern "system" fn(i32) -> i32),
                error: f!(
                    "Everything_GetLastError",
                    unsafe extern "system" fn() -> u32
                ),
                total: f!(
                    "Everything_GetTotResults",
                    unsafe extern "system" fn() -> u32
                ),
                count: f!(
                    "Everything_GetNumResults",
                    unsafe extern "system" fn() -> u32
                ),
                path: f!(
                    "Everything_GetResultFullPathNameW",
                    unsafe extern "system" fn(u32, *mut u16, u32) -> u32
                ),
                folder: f!(
                    "Everything_IsFolderResult",
                    unsafe extern "system" fn(u32) -> i32
                ),
                size: f!(
                    "Everything_GetResultSize",
                    unsafe extern "system" fn(u32, *mut i64) -> i32
                ),
                created: f!(
                    "Everything_GetResultDateCreated",
                    unsafe extern "system" fn(u32, *mut u64) -> i32
                ),
                modified: f!(
                    "Everything_GetResultDateModified",
                    unsafe extern "system" fn(u32, *mut u64) -> i32
                ),
            })
        }
    }
}
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
pub fn format_time(ticks: u64) -> String {
    #[cfg(windows)]
    {
        if ticks == 0 {
            return "—".into();
        }
        #[repr(C)]
        #[derive(Default)]
        struct Calendar {
            year: u16,
            month: u16,
            weekday: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            millis: u16,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn FileTimeToLocalFileTime(input: *const u64, output: *mut u64) -> i32;
            fn FileTimeToSystemTime(input: *const u64, output: *mut Calendar) -> i32;
        }
        let mut local = 0;
        let mut date = Calendar::default();
        unsafe {
            if FileTimeToLocalFileTime(&ticks, &mut local) != 0
                && FileTimeToSystemTime(&local, &mut date) != 0
            {
                return format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    date.year, date.month, date.day, date.hour, date.minute, date.second
                );
            }
        }
    }
    "—".into()
}
pub fn start() -> Result<()> {
    for dir in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("Programs").into_os_string()),
    ]
    .into_iter()
    .flatten()
    {
        let exe = PathBuf::from(dir).join("Everything/Everything.exe");
        if exe.is_file() {
            let mut command = std::process::Command::new(exe);
            command.arg("-startup");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            command.spawn()?;
            return Ok(());
        }
    }
    anyhow::bail!("Everything was not found. Install or start Everything from voidtools.com; local filtering works without it.")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obsolete_reply_is_ignored() {
        let mut client=Client::default();client.cancel();client.busy=true;
        *client.reply.lock().unwrap()=Some((0,Err("obsolete".into())));
        assert!(client.poll().is_none());assert!(client.busy);
        *client.reply.lock().unwrap()=Some((1,Err("current".into())));
        assert!(client.poll().unwrap().is_err());assert!(!client.busy);
    }
    #[test]
    #[ignore = "manual: requires a running Everything client with an index"]
    fn indexed_search_smoke() {
        let result = query(
            "ext:rs path:C:\\Users\\eugene\\Documents\\_tool_viewer\\src",
            0,
        )
        .unwrap();
        assert!(result.total > 0);
        assert!(result
            .hits
            .iter()
            .any(|hit| hit.path.file_name().is_some_and(|name| name == "main.rs")));
        println!(
            "Everything integration: {} matches; {:?}",
            result.total, result.elapsed
        );
    }
}
