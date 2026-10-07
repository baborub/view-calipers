use std::{hash::{Hash, Hasher}, path::{Path, PathBuf}, sync::atomic::{AtomicU64, Ordering}};
use anyhow::Result;
use crate::ImageSrc;

static EPOCH: AtomicU64 = AtomicU64::new(0);
pub fn epoch() -> u64 { EPOCH.load(Ordering::Relaxed) }
pub fn directory() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("ArchView").join("thumbnails-v1")
}
pub fn key(source: &ImageSrc, root: &Path) -> Result<PathBuf> {
    let (path, name) = match source {
        ImageSrc::File(path) => (path, ""),
        ImageSrc::Entry { archive, name } => (archive, name.as_str()),
    };
    let metadata = std::fs::metadata(path)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    "320x240-exif-v2".hash(&mut hash);
    std::fs::canonicalize(path)?.hash(&mut hash);
    name.hash(&mut hash);
    metadata.len().hash(&mut hash);
    metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().hash(&mut hash);
    Ok(root.join(format!("{:016x}.png", hash.finish())))
}
pub fn load(path: &Path) -> Option<image::RgbaImage> { image::open(path).ok().map(|image| image.to_rgba8()) }
pub fn save(path: &Path, image: &image::RgbaImage, epoch: u64) -> Result<()> {
    if epoch != self::epoch() { return Ok(()); }
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_extension(format!("{}-{stamp}.tmp", std::process::id()));
    image.save_with_format(&temporary, image::ImageFormat::Png)?;
    if epoch != self::epoch() { let _ = std::fs::remove_file(temporary); return Ok(()); }
    if let Err(error) = std::fs::rename(&temporary, path) { let _ = std::fs::remove_file(temporary); return Err(error.into()); }
    Ok(())
}
pub fn clear() -> Result<usize> {
    EPOCH.fetch_add(1, Ordering::Relaxed);
    let root = directory();
    if !root.exists() { return Ok(0); }
    let mut count = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() { std::fs::remove_file(entry.path())?; count += 1; }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_cache_roundtrip_and_source_change_invalidate_key() {
        let root = std::env::temp_dir().join(format!("archview-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("document.md");
        std::fs::write(&source, "# Original").unwrap();
        let src = ImageSrc::File(source.clone());
        let key_before = key(&src, &root).unwrap();
        let image = image::RgbaImage::from_pixel(16, 12, image::Rgba([20, 40, 60, 255]));
        save(&key_before, &image, epoch()).unwrap();
        assert_eq!(load(&key_before).unwrap(), image);
        std::fs::write(&source, "# A longer edited document").unwrap();
        let key_after = key(&src, &root).unwrap();
        assert_ne!(key_before, key_after);
        assert!(load(&key_after).is_none());
        std::fs::write(&key_after, "broken cache entry").unwrap();
        assert!(load(&key_after).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
