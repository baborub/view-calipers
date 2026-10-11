//! Filesystem jobs never overwrite existing destinations. No media decoding runs here.
use anyhow::{bail, Context, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub enum Job {
    Copy(Vec<PathBuf>, PathBuf),
    Move(Vec<PathBuf>, PathBuf),
    Duplicate(Vec<PathBuf>),
    Recycle(Vec<PathBuf>),
    Rename(PathBuf, String),
    NewFolder(PathBuf, String),
}
#[derive(Default)]
pub struct Outcome {
    pub paths: Vec<PathBuf>,
    pub moved: Vec<PathBuf>,
    pub changed: Vec<PathBuf>,
    pub errors: Vec<String>,
}

pub fn valid_name(name: &str) -> Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
        || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        bail!("Use a valid Windows file name (without a path).");
    }
    Ok(())
}
fn unique(source: &Path, directory: &Path, duplicate: bool) -> Result<PathBuf> {
    let name = source
        .file_name()
        .context("Drive roots cannot be copied or moved")?;
    let direct = directory.join(name);
    if !duplicate && !direct.exists() {
        return Ok(direct);
    }
    let stem = if source.is_dir() {
        name.to_string_lossy().into_owned()
    } else {
        source
            .file_stem()
            .unwrap_or(name)
            .to_string_lossy()
            .into_owned()
    };
    let extension = if source.is_dir() {
        String::new()
    } else {
        source
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default()
    };
    for n in 1..100000 {
        let suffix = if n == 1 {
            " - Copy".into()
        } else {
            format!(" - Copy ({n})")
        };
        let target = directory.join(format!("{stem}{suffix}{extension}"));
        if !target.exists() {
            return Ok(target);
        }
    }
    bail!("Too many copies with the same name")
}
fn copy(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("Links and junctions are not copied: {}", source.display());
        }
    }
    if metadata.file_type().is_symlink() {
        bail!("Symbolic links are not copied");
    }
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        output.set_times(fs::FileTimes::new().set_modified(metadata.modified()?))?;
    }
    Ok(())
}
fn transfer(source: &Path, directory: &Path, moving: bool, duplicate: bool) -> Result<PathBuf> {
    let directory = fs::canonicalize(directory).context("Destination folder is unavailable")?;
    let metadata = fs::symlink_metadata(source)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("Links and junctions cannot be transferred");
        }
    }
    let source = fs::canonicalize(source).context("Source is unavailable")?;
    if !directory.is_dir() {
        bail!("Destination must be a folder");
    }
    if source.is_dir() && directory.starts_with(&source) {
        bail!("Cannot put a folder inside itself");
    }
    if moving && source.parent() == Some(directory.as_path()) {
        bail!("Already in the destination folder");
    }
    let target = unique(&source, &directory, duplicate)?;
    if moving {
        // Our Windows rename does not replace an existing file. Only a different-drive
        // error permits the copy-then-delete fallback; permission errors must surface.
        match rename(&source, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.raw_os_error() == Some(17) => {}
            Err(e) => return Err(e.into()),
        }
    }
    copy(&source, &target).with_context(|| {
        format!(
            "Copy failed; partial destination may remain at {}. Source retained",
            target.display()
        )
    })?;
    if moving {
        if source.is_dir() {
            fs::remove_dir_all(&source)?;
        } else {
            fs::remove_file(&source)?;
        }
    }
    Ok(target)
}
fn rename(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    fn attempt(source: &Path, target: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // No MOVEFILE_REPLACE_EXISTING: even a concurrent name collision is safe.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    fn attempt(source: &Path, target: &Path) -> io::Result<()> {
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination exists",
            ));
        }
        fs::rename(source, target)
    }
    for retry in 0..20 {
        match attempt(source, target) {
            Err(e) if e.raw_os_error() == Some(32) && retry < 19 => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            result => return result,
        }
    }
    unreachable!()
}
pub fn run(job: Job) -> Outcome {
    let mut outcome = Outcome::default();
    let (sources, destination, moving, duplicate) = match job {
        Job::Copy(p, d) => (p, Some(d), false, false),
        Job::Move(p, d) => (p, Some(d), true, false),
        Job::Duplicate(p) => (p, None, false, true),
        Job::Recycle(sources) => {
            for source in sources {
                if let Some(parent) = source.parent() {
                    outcome.changed.push(parent.into());
                }
                match recycle(&source) {
                    Ok(()) => {
                        outcome.paths.push(source.clone());
                        outcome.moved.push(source);
                    }
                    Err(e) => outcome.errors.push(format!("{}: {e:#}", source.display())),
                }
            }
            return outcome;
        }
        Job::NewFolder(directory,name) => {
            let result=(|| {valid_name(&name)?;let target=directory.join(name);fs::create_dir(&target)?;Ok::<_,anyhow::Error>(target)})();
            match result {Ok(path)=>{outcome.changed.push(directory);outcome.paths.push(path);},Err(error)=>outcome.errors.push(format!("Create folder: {error:#}"))}
            return outcome;
        }
        Job::Rename(source, name) => {
            let result = (|| {
                valid_name(&name)?;
                let parent = source.parent().context("Cannot rename a drive")?;
                let target = parent.join(name);
                if target == source {
                    return Ok(target);
                }
                if target.exists() {
                    bail!("A file with that name already exists");
                }
                rename(&source, &target)?;
                Ok(target)
            })();
            if let Some(parent) = source.parent() {
                outcome.changed.push(parent.into());
            }
            match result {
                Ok(p) => outcome.paths.push(p),
                Err(e) => outcome.errors.push(format!("{}: {e:#}", source.display())),
            }
            return outcome;
        }
    };
    for source in sources {
        let directory = destination.as_deref().or(source.parent());
        if let Some(parent) = source.parent() {
            outcome.changed.push(parent.into());
        }
        if let Some(directory) = directory {
            outcome.changed.push(directory.into());
            match transfer(&source, directory, moving, duplicate) {
                Ok(p) => {
                    outcome.paths.push(p);
                    if moving {
                        outcome.moved.push(source.clone());
                    }
                }
                Err(e) => outcome.errors.push(format!("{}: {e:#}", source.display())),
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_preserve_sources_and_collisions() {
        let root = std::env::temp_dir().join(format!(
            "view-calipers-fileops-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let source = a.join("hello.bin");
        fs::write(&source, b"payload").unwrap();
        fs::write(b.join("hello.bin"), b"existing").unwrap();
        let result = run(Job::Copy(vec![source.clone()], b.clone()));
        assert!(result.errors.is_empty());
        assert_eq!(fs::read(&result.paths[0]).unwrap(), b"payload");
        assert_eq!(fs::read(b.join("hello.bin")).unwrap(), b"existing");
        let result = run(Job::Duplicate(vec![source.clone()]));
        assert!(result.errors.is_empty());
        assert!(source.exists());
        let result = run(Job::Rename(source.clone(), "renamed.bin".into()));
        assert!(result.errors.is_empty());
        assert!(!source.exists());
        let renamed = a.join("renamed.bin");
        let result = run(Job::Move(vec![renamed.clone()], b.clone()));
        assert!(result.errors.is_empty());
        assert!(!renamed.exists());
        assert_eq!(fs::read(b.join("renamed.bin")).unwrap(), b"payload");
        let nested = a.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("f"), b"x").unwrap();
        assert!(!run(Job::Copy(vec![a.clone()], nested.clone()))
            .errors
            .is_empty());
        assert!(!run(Job::Rename(b.join("renamed.bin"), "hello.bin".into()))
            .errors
            .is_empty());
        assert!(!run(Job::Rename(b.join("renamed.bin"), "../escape".into()))
            .errors
            .is_empty());
        assert!(run(Job::Copy(vec![nested], b)).errors.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn native_rename_never_replaces_existing_destination() {
        let root =
            std::env::temp_dir().join(format!("view-calipers-no-replace-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        assert!(rename(&a, &b).is_err());
        assert_eq!(fs::read(&a).unwrap(), b"a");
        assert_eq!(fs::read(&b).unwrap(), b"b");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "manual: set VIEW_CALIPERS_OTHER_DRIVE to a writable directory on another drive"]
    fn cross_drive_move() {
        let base = PathBuf::from(
            std::env::var_os("VIEW_CALIPERS_OTHER_DRIVE").expect("Destination test base"),
        );
        let id = format!(
            "view-calipers-transfer-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source = std::env::temp_dir().join(&id);
        let dest = base.join(&id);
        assert_ne!(source.components().next(), dest.components().next());
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        let file = source.join("payload.dat");
        fs::write(&file, vec![7u8; 1024 * 1024]).unwrap();
        let result = run(Job::Move(vec![file.clone()], dest.clone()));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(!file.exists());
        assert_eq!(
            fs::read(dest.join("payload.dat")).unwrap(),
            vec![7u8; 1024 * 1024]
        );
        let dir = source.join("folder");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("child"), b"child").unwrap();
        let result = run(Job::Move(vec![dir.clone()], dest.clone()));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(!dir.exists());
        assert_eq!(fs::read(dest.join("folder/child")).unwrap(), b"child");
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(dest).unwrap();
    }
}

/// Use the Windows shell's recoverable deletion; do not implement a permanent-delete fallback.
fn recycle(source: &Path) -> Result<()> {
    if !source.is_absolute() || source.file_name().is_none() {
        bail!("Select a file or folder, not a drive root");
    }
    let metadata = fs::symlink_metadata(source).context("Item is unavailable")?;
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("Links and junctions cannot be recycled here");
        }
        // SHFileOperation does not accept extended path prefixes or wildcard sources.
        let text = source.to_string_lossy();
        if text.starts_with(r"\\?\") || text.contains(['*', '?']) {
            bail!("This path is not supported by the Windows recycle operation");
        }
        #[repr(C)]
        struct Operation {
            window: *mut std::ffi::c_void,
            kind: u32,
            from: *const u16,
            to: *const u16,
            flags: u16,
            aborted: i32,
            mappings: *mut std::ffi::c_void,
            title: *const u16,
        }
        #[link(name = "shell32")]
        extern "system" {
            fn SHFileOperationW(operation: *mut Operation) -> i32;
        }
        #[link(name = "ole32")]
        extern "system" {
            fn CoInitializeEx(reserved: *mut std::ffi::c_void, flags: u32) -> i32;
            fn CoUninitialize();
        }
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let initialized = unsafe { CoInitializeEx(std::ptr::null_mut(), 2) };
        if initialized < 0 {
            bail!("Could not initialize Windows recycle operation ({initialized})");
        }
        let _com = Com;
        let from: Vec<u16> = source.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut operation = Operation {
            window: std::ptr::null_mut(),
            kind: 3,
            from: from.as_ptr(),
            to: std::ptr::null(),
            flags: 0x0040 | 0x0010 | 0x0400 | 0x4000,
            aborted: 0,
            mappings: std::ptr::null_mut(),
            title: std::ptr::null(),
        };
        // ALLOWUNDO, NOCONFIRMATION, NOERRORUI, WANTNUKEWARNING: Windows must warn
        // if a destination volume cannot recycle and would permanently destroy an item.
        let result = unsafe { SHFileOperationW(&mut operation) };
        if result != 0 || operation.aborted != 0 {
            bail!("Recycle cancelled or failed (Windows shell code {result})");
        }
        if source.exists() {
            bail!("The item was not recycled");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        bail!("Recycle is available on Windows only")
    }
}
#[cfg(test)]
mod recycle_tests {
    use super::*;
    #[test]
    fn recycle_rejects_roots_relative_paths_and_missing_items() {
        assert!(recycle(Path::new("C:\\")).is_err());
        assert!(recycle(Path::new("relative.txt")).is_err());
        assert!(recycle(&std::env::temp_dir().join("view-calipers-missing-recycle-item")).is_err());
    }
    #[test]
    fn recycle_synthetic_file_and_folder_refreshes_sources() {
        let root =
            std::env::temp_dir().join(format!("view-calipers-recycle-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("synthetic.txt");
        let folder = root.join("synthetic-folder");
        fs::write(&file, b"disposable recycle fixture").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("child.txt"), b"disposable").unwrap();
        let outcome = run(Job::Recycle(vec![file.clone(), folder.clone()]));
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.paths.len(), 2);
        assert!(!file.exists() && !folder.exists());
        assert!(outcome.changed.contains(&root));
        fs::remove_dir(root).unwrap();
    }
}
