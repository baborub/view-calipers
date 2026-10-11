use super::*;
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn busy() -> bool {
    BUSY.load(AtomicOrdering::Relaxed)
}

pub fn default_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir()
            .join(format!("view-calipers-library-test-{}", std::process::id()));
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::{
            FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        };
        if let Ok(path) = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) {
            let value = path.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(path.0.cast()));
            if let Some(value) = value {
                return PathBuf::from(value).join("view-calipers");
            }
        }
    }
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Documents/view-calipers")
}
pub fn root() -> PathBuf {
    store().lock().unwrap().root.clone()
}
pub fn error() -> Option<String> {
    store().lock().unwrap().error.clone()
}
pub fn board_path(id: &str) -> Result<PathBuf> {
    anyhow::ensure!(!busy(), "Wait for the library copy to finish");
    let s = store().lock().unwrap();
    if let Some(error) = &s.error {
        anyhow::bail!("Collections: {error}");
    }
    let c = s
        .lib
        .collections
        .iter()
        .find(|c| c.id == id)
        .context("Collection no longer exists")?;
    Ok(board_at(&s.root, c))
}
fn board_at(root: &Path, c: &Collection) -> PathBuf {
    root.join(&c.folder).join(format!("{}.vcboard", c.folder))
}
pub fn legacy_board(id: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    settings::Settings::path()
        .with_file_name("boards")
        .join(format!("collection-{:016x}.vcboard", hash.finish()))
}
fn safe_legacy_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_end_matches(['.', ' ']);
    if fileops::valid_name(name).is_ok() {
        name.into()
    } else {
        "Collection".into()
    }
}
fn save_index(root: &Path, lib: &Library) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let file = root.join("collections.tsv");
    let temporary = file.with_extension("tmp");
    std::fs::write(&temporary, lib.text())?;
    std::fs::rename(temporary, file)?;
    Ok(())
}
pub(super) fn initialize() -> Store {
    let configured = if cfg!(test) {
        String::new()
    } else {
        settings::Settings::load().collections_root
    };
    let root = if configured.is_empty() {
        default_root()
    } else {
        PathBuf::from(configured)
    };
    let load = || -> Result<Library> {
        let index = root.join("collections.tsv");
        if index.exists() {
            let lib = Library::parse(&std::fs::read_to_string(index)?)?;
            for c in &lib.collections {
                fileops::valid_name(&c.folder)?;
            }
            return Ok(lib);
        }
        let mut lib = match std::fs::read_to_string(path()) {
            Ok(text) => Library::parse(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Library::default(),
            Err(e) => return Err(e.into()),
        };
        migrate(&root, &mut lib, legacy_board)?;
        Ok(lib)
    };
    match load() {
        Ok(lib) => Store {
            lib,
            root,
            error: None,
        },
        Err(e) => Store {
            lib: Library::default(),
            root,
            error: Some(e.to_string()),
        },
    }
}
pub(super) fn migrate(
    root: &Path,
    lib: &mut Library,
    old_board: impl Fn(&str) -> PathBuf,
) -> Result<()> {
    std::fs::create_dir_all(root)?;
    for c in &mut lib.collections {
        let base = safe_legacy_name(&c.name);
        let mut folder = base.clone();
        let mut suffix = 2;
        loop {
            let dir = root.join(&folder);
            if !dir.exists()
                || std::fs::read_to_string(dir.join(".collection-id"))
                    .ok()
                    .as_deref()
                    == Some(&c.id)
            {
                break;
            }
            folder = format!("{base} ({suffix})");
            suffix += 1;
        }
        c.folder = folder;
        c.name = c.folder.clone();
        let target = board_at(root, c);
        let old = old_board(&c.id);
        if !target.exists() {
            let mut doc = if old.exists() {
                board::read(&old)?
            } else {
                board::Document::new(c.name.clone())
            };
            if old.exists() {
                doc.rebase(&old, &target);
            } else {
                doc.add_images(&c.paths, &target, Pos2::ZERO);
            }
            doc.title = c.name.clone();
            board::write(&target, &doc)?;
        }
        std::fs::write(target.parent().unwrap().join(".collection-id"), &c.id)?;
    }
    save_index(root, lib)
}
pub(super) fn prepare(c: &Collection, root: &Path) -> Result<()> {
    fileops::valid_name(&c.folder)?;
    let dir = root.join(&c.folder);
    // Never reuse someone else's folder or overwrite an existing collection.
    std::fs::create_dir_all(root)?;
    std::fs::create_dir(&dir).context("A folder with this collection name already exists")?;
    let path = board_at(root, c);
    let mut doc = board::Document::new(c.name.clone());
    doc.add_images(&c.paths, &path, Pos2::ZERO);
    board::write(&path, &doc)?;
    std::fs::write(dir.join(".collection-id"), &c.id)?;
    Ok(())
}
pub fn rename(id: &str, name: &str) -> Result<()> {
    anyhow::ensure!(!busy(), "Wait for the library copy to finish");
    let name = name.trim();
    fileops::valid_name(name)?;
    let mut s = store().lock().unwrap();
    anyhow::ensure!(s.error.is_none(), "Library is unavailable");
    let mut next = s.lib.clone();
    let c = next
        .collections
        .iter_mut()
        .find(|c| c.id == id)
        .context("Collection missing")?;
    if c.name == name {
        return Ok(());
    }
    let old_board = board_at(&s.root, c);
    let old_dir = std::fs::canonicalize(old_board.parent().unwrap())?;
    let root = std::fs::canonicalize(&s.root)?;
    anyhow::ensure!(
        old_dir.parent() == Some(root.as_path()),
        "Collection folder is outside its library"
    );
    let target = root.join(name);
    anyhow::ensure!(!target.exists(), "A folder with this name already exists");
    let old_name = c.folder.clone();
    let original_doc = board::read(&old_board)?;
    c.name = name.into();
    c.folder = name.into();
    // A rename is confined to a verified immediate child of the library.
    std::fs::rename(&old_dir, &target)?;
    let moved_board = target.join(format!("{old_name}.vcboard"));
    let new_board = target.join(format!("{name}.vcboard"));
    if let Err(e) = std::fs::rename(&moved_board, &new_board) {
        let _ = std::fs::rename(&target, &old_dir);
        return Err(e.into());
    }
    let mut doc = original_doc.clone();
    doc.title = name.into();
    // Managed sources are relative. Also rebase absolute references to owned images.
    doc.relocate_library(&old_board, &new_board, &old_dir, &target);
    if let Err(e) = board::write(&new_board, &doc).and_then(|_| save_index(&s.root, &next)) {
        let _ = board::write(&new_board, &original_doc);
        let _ = std::fs::rename(&new_board, &moved_board);
        let _ = std::fs::rename(&target, &old_dir);
        return Err(e);
    }
    next.revision += 1;
    s.lib = next;
    Ok(())
}
pub fn relocate(target: &Path) -> Result<()> {
    anyhow::ensure!(target.is_absolute(), "Choose an absolute library path");
    anyhow::ensure!(
        BUSY.compare_exchange(
            false,
            true,
            AtomicOrdering::Relaxed,
            AtomicOrdering::Relaxed
        )
        .is_ok(),
        "Library copy is already running"
    );
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            BUSY.store(false, AtomicOrdering::Relaxed);
        }
    }
    let _guard = Guard;
    let s = store().lock().unwrap();
    anyhow::ensure!(s.error.is_none(), "Library is unavailable");
    let lib = s.lib.clone();
    let source_root = s.root.clone();
    drop(s);
    std::fs::create_dir_all(target)?;
    let target = std::fs::canonicalize(target)?;
    let old = std::fs::canonicalize(&source_root)?;
    if old == target {
        return Ok(());
    }
    anyhow::ensure!(
        !target.starts_with(&old) && !old.starts_with(&target),
        "Choose a folder outside the current library"
    );
    anyhow::ensure!(
        std::fs::read_dir(&target)?.next().is_none(),
        "Choose an empty folder for the library"
    );
    copy_tree(&old, &target)?;
    // All managed images use relative paths. External references stay absolute.
    for c in &lib.collections {
        let from = board_at(&old, c);
        let to = board_at(&target, c);
        let mut doc = board::read(&to)?;
        doc.relocate_library(&from, &to, &old, &target);
        board::write(&to, &doc)?;
    }
    save_index(&target, &lib)?;
    let mut s = store().lock().unwrap();
    s.root = target;
    s.lib.revision += 1;
    Ok(())
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            anyhow::ensure!(
                entry.metadata()?.file_attributes() & 0x400 == 0,
                "Library contains a junction or link: {}",
                entry.path().display()
            );
        }
        anyhow::ensure!(
            !ty.is_symlink(),
            "Library contains a symbolic link: {}",
            entry.path().display()
        );
        let target = to.join(entry.file_name());
        if ty.is_dir() {
            std::fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
