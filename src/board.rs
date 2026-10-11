//! Reference boards store references and editable objects, never modified source pixels.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, Weak};
use std::time::{Duration, Instant};
mod assets;
mod editor;
mod integration;
mod markdown;
mod media;
mod operations;
pub use editor::Editor;
static IMPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn imports_running() -> bool {
    IMPORTS.load(AtomicOrdering::Relaxed) > 0
}

/// Archive entries are references, not filesystem paths that file operations may move.
#[derive(Clone)]
pub(crate) struct ImageDrag {
    pub sources: Vec<ImageSrc>,
}

#[derive(Clone)]
pub(crate) struct RevealSource {
    pub source: ImageSrc,
    pub board: PathBuf,
}

const MAX_OBJECTS: usize = 10_000;
const MAX_POINTS: usize = 500_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Image {
        source: PathBuf,
    },
    Video {
        source: PathBuf,
    },
    Markdown {
        source: PathBuf,
    },
    ArchiveImage {
        archive: PathBuf,
        entry: String,
    },
    Text {
        text: String,
        color: [u8; 4],
    },
    Ink {
        points: Vec<[f32; 2]>,
        color: [u8; 4],
        width: f32,
    },
}
impl Content {
    fn source_path_mut(&mut self) -> Option<&mut PathBuf> {
        match self {
            Self::Image { source } | Self::Video { source } | Self::Markdown { source } => {
                Some(source)
            }
            Self::ArchiveImage { archive, .. } => Some(archive),
            _ => None,
        }
    }
    fn image_source(&self, board: &Path) -> Option<ImageSrc> {
        match self {
            Self::Image { source } => Some(ImageSrc::File(resolve(source, board))),
            Self::ArchiveImage { archive, entry } => Some(ImageSrc::Entry {
                archive: resolve(archive, board),
                name: entry.clone(),
            }),
            _ => None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Object {
    pub id: u64,
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub angle: f32,
    pub content: Content,
    #[serde(default)]
    pub options: operations::ObjectOptions,
}
impl Object {
    pub fn center(&self) -> Pos2 {
        Pos2::new(self.center[0], self.center[1])
    }
    pub fn corners(&self) -> [Pos2; 4] {
        let half = Vec2::new(self.size[0], self.size[1]) * 0.5;
        [
            Vec2::new(-half.x, -half.y),
            Vec2::new(half.x, -half.y),
            half,
            Vec2::new(-half.x, half.y),
        ]
        .map(|p| self.center() + rotate(p, self.angle))
    }
    pub fn bounds(&self) -> Rect {
        Rect::from_points(&self.corners())
    }
    pub fn hit(&self, point: Pos2, tolerance: f32) -> bool {
        let p = rotate(point - self.center(), -self.angle);
        p.x.abs() <= self.size[0] * 0.5 + tolerance && p.y.abs() <= self.size[1] * 0.5 + tolerance
    }
}
pub fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
pub struct Camera {
    pub center: [f32; 2],
    pub zoom: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            zoom: 1.0,
        }
    }
}
impl Camera {
    pub fn screen(self, p: Pos2, area: Rect) -> Pos2 {
        area.center() + (p - Pos2::new(self.center[0], self.center[1])) * self.zoom
    }
    pub fn world(self, p: Pos2, area: Rect) -> Pos2 {
        Pos2::new(self.center[0], self.center[1]) + (p - area.center()) / self.zoom
    }
    pub fn zoom_at(&mut self, pointer: Pos2, area: Rect, factor: f32) {
        let before = self.world(pointer, area);
        self.zoom = (self.zoom * factor).clamp(0.005, 32.0);
        let delta = before - self.world(pointer, area);
        self.center[0] += delta.x;
        self.center[1] += delta.y;
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Document {
    pub version: u32,
    pub title: String,
    pub camera: Camera,
    pub objects: Vec<Object>,
    #[serde(default)]
    pub groups: std::collections::BTreeMap<u64, String>,
    #[serde(default)]
    pub canvas: operations::CanvasOptions,
}
impl Document {
    pub fn rebase(&mut self, from: &Path, to: &Path) {
        for obj in &mut self.objects {
            if let Some(source) = obj.content.source_path_mut() {
                *source = reference(&resolve(source, from), to);
            }
        }
    }
    pub fn relocate_library(&mut self, from: &Path, to: &Path, old_root: &Path, new_root: &Path) {
        for obj in &mut self.objects {
            if let Some(source) = obj.content.source_path_mut() {
                if source.is_relative() {
                    continue;
                }
                let absolute = resolve(source, from);
                let normalized = std::fs::canonicalize(&absolute).unwrap_or(absolute.clone());
                *source = if let Ok(rel) = normalized.strip_prefix(old_root) {
                    reference(&new_root.join(rel), to)
                } else {
                    reference(&absolute, to)
                };
            }
        }
    }
    pub fn new(title: String) -> Self {
        Self {
            version: 1,
            title,
            camera: Camera::default(),
            objects: vec![],
            groups: Default::default(),
            canvas: Default::default(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            matches!(self.version, 1 | 2 | 3),
            "Unsupported board version {}",
            self.version
        );
        anyhow::ensure!(
            self.objects.len() <= MAX_OBJECTS,
            "Board has too many objects"
        );
        anyhow::ensure!(self.title.len() <= 8192, "Board title is too long");
        self.validate_options()?;
        anyhow::ensure!(
            self.camera
                .center
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1e9)
                && self.camera.zoom.is_finite()
                && (0.005..=32.0).contains(&self.camera.zoom),
            "Invalid camera"
        );
        let mut ids = HashSet::new();
        let mut points = 0;
        for object in &self.objects {
            anyhow::ensure!(
                ids.insert(object.id) && object.id < u64::MAX,
                "Invalid or duplicate object ID"
            );
            anyhow::ensure!(
                object
                    .center
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 1e9)
                    && object
                        .size
                        .iter()
                        .all(|v| v.is_finite() && (0.1..=1e7).contains(v))
                    && object.angle.is_finite(),
                "Invalid object transform"
            );
            match &object.content {
                Content::Image { source }
                | Content::Video { source }
                | Content::Markdown { source } => {
                    anyhow::ensure!(!source.as_os_str().is_empty(), "Empty media reference")
                }
                Content::ArchiveImage { archive, entry } => {
                    anyhow::ensure!(
                        !archive.as_os_str().is_empty()
                            && !entry.is_empty()
                            && entry.len() <= 32768
                            && is_image_ext(&ext_of_name(entry)),
                        "Invalid archive image reference"
                    );
                }
                Content::Text { text, .. } => {
                    anyhow::ensure!(text.len() <= 1_000_000, "Text note is too large")
                }
                Content::Ink {
                    points: p, width, ..
                } => {
                    points += p.len();
                    anyhow::ensure!(
                        points <= MAX_POINTS
                            && width.is_finite()
                            && (0.1..=100.0).contains(width)
                            && p.iter().flatten().all(|v| v.is_finite() && v.abs() <= 1.0),
                        "Invalid ink stroke"
                    );
                }
            }
        }
        Ok(())
    }
    pub fn next_id(&self) -> u64 {
        self.objects.iter().map(|o| o.id).max().unwrap_or(0) + 1
    }
    pub fn add_images(&mut self, paths: &[PathBuf], board_path: &Path, origin: Pos2) -> Vec<u64> {
        self.add_sources(
            &paths
                .iter()
                .cloned()
                .map(ImageSrc::File)
                .collect::<Vec<_>>(),
            board_path,
            origin,
        )
    }
    pub fn add_sources(
        &mut self,
        sources: &[ImageSrc],
        board_path: &Path,
        origin: Pos2,
    ) -> Vec<u64> {
        let mut added = vec![];
        for source in sources.iter().filter(|source| supports_source(source)) {
            if self.objects.len() >= MAX_OBJECTS {
                break;
            }
            let index = added.len();
            let id = self.next_id();
            // Header-only dimensions keep adding references inexpensive; decoded EXIF orientation
            // is applied by the renderer and image fitting preserves its aspect ratio.
            // Archive I/O stays in the background thumbnail/decode workers.
            let kind = match source {
                ImageSrc::File(p) => media_kind(p),
                _ => None,
            };
            if kind.is_some() {
                self.version = self.version.max(2);
            }
            let (w, h) = match source {
                ImageSrc::File(path) if kind.is_none() => {
                    image::image_dimensions(path).unwrap_or((4, 3))
                }
                ImageSrc::File(_) => (16, 9),
                ImageSrc::Entry { .. } => (4, 3),
            };
            let size = match kind {
                Some(MediaKind::Markdown) => [420.0, 520.0],
                Some(MediaKind::Video) => [420.0, 236.25],
                None => [
                    300.0,
                    (300.0 * h as f32 / w.max(1) as f32).clamp(80.0, 400.0),
                ],
            };
            self.objects.push(Object {
                id,
                center: [
                    origin.x + (index % 3) as f32 * 460.0,
                    origin.y + (index / 3) as f32 * 560.0,
                ],
                size,
                angle: 0.0,
                options: Default::default(),
                content: match source {
                    ImageSrc::File(path) => match kind {
                        Some(MediaKind::Video) => Content::Video {
                            source: reference(path, board_path),
                        },
                        Some(MediaKind::Markdown) => Content::Markdown {
                            source: reference(path, board_path),
                        },
                        None => Content::Image {
                            source: reference(path, board_path),
                        },
                    },
                    ImageSrc::Entry { archive, name } => Content::ArchiveImage {
                        archive: reference(archive, board_path),
                        entry: name.clone(),
                    },
                },
            });
            added.push(id);
        }
        added
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MediaKind {
    Video,
    Markdown,
}
fn media_kind(path: &Path) -> Option<MediaKind> {
    let ext = ext_of_path(path);
    if VIDEO_EXT.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else if ["md", "markdown"].contains(&ext.as_str()) {
        Some(MediaKind::Markdown)
    } else {
        None
    }
}
pub(crate) fn supports_source(source: &ImageSrc) -> bool {
    image_pipeline::is_photo(source)
        || matches!(source, ImageSrc::File(p) if media_kind(p).is_some())
}
pub fn resolve(source: &Path, board: &Path) -> PathBuf {
    if source.is_absolute() {
        source.into()
    } else {
        board.parent().unwrap_or(Path::new(".")).join(source)
    }
}
pub fn reference(source: &Path, board: &Path) -> PathBuf {
    let parent = board.parent().unwrap_or(Path::new("."));
    if let Ok(relative) = source.strip_prefix(parent) {
        return relative.into();
    }
    // Open editors use canonical Windows paths (\\?\C:\...), while browser drag
    // sources normally use C:\... . Compare equivalent paths before falling back.
    if let (Ok(source), Ok(parent)) = (std::fs::canonicalize(source), std::fs::canonicalize(parent))
    {
        if let Ok(relative) = source.strip_prefix(parent) {
            return relative.into();
        }
    }
    source.into()
}
pub fn read(path: &Path) -> Result<Document> {
    anyhow::ensure!(
        std::fs::metadata(path)?.len() <= MAX_FILE_BYTES,
        "Board exceeds 64 MiB"
    );
    let doc: Document =
        serde_json::from_reader(File::open(path)?).context("Invalid .vcboard file")?;
    doc.validate()?;
    Ok(doc)
}
fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}
pub fn write(path: &Path, doc: &Document) -> Result<()> {
    use std::io::Write;
    doc.validate()?;
    let parent = path.parent().context("Board needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "vcboard.{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let data = serde_json::to_vec_pretty(doc)?;
        anyhow::ensure!(data.len() as u64 <= MAX_FILE_BYTES, "Board exceeds 64 MiB");
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

struct Shared {
    path: PathBuf,
    doc: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    dirty: bool,
    due: Instant,
    saved: Option<(u64, std::time::SystemTime)>,
    error: Option<String>,
    // Only one preview may own an unfinished gesture on the same document.
    gesture: Option<u64>,
}
impl Shared {
    fn changed(&mut self) {
        self.doc.upgrade_options();
        self.dirty = true;
        self.due = Instant::now() + Duration::from_millis(650);
        self.error = None;
    }
    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        self.redo.clear();
        // Bound history by both count and approximate serialized bytes.
        while self.undo.len() > 50
            || (self.undo.len() > 1
                && self
                    .undo
                    .iter()
                    .map(|d| serde_json::to_vec(d).map_or(0, |v| v.len()))
                    .sum::<usize>()
                    > 32 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
    }
    fn undo(&mut self, redo: bool) {
        if self.gesture.is_some() {
            return;
        }
        let previous = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(previous) = previous {
            let current = std::mem::replace(&mut self.doc, previous);
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.changed();
        }
    }
    fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let result = (|| {
            anyhow::ensure!(
                stamp(&self.path) == self.saved,
                "Board changed outside this session. Use Save as to preserve your edits."
            );
            write(&self.path, &self.doc)
        })();
        match result {
            Ok(()) => {
                self.saved = stamp(&self.path);
                self.dirty = false;
                self.error = None;
                Ok(())
            }
            Err(e) => {
                self.error = Some(e.to_string());
                Err(e)
            }
        }
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        if self.flush().is_err() {
            // Preserve unsaved edits even when the source is read-only or was edited externally.
            let recovery = settings::Settings::path().with_file_name("board-recovery");
            let time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let target = recovery.join(format!("recovered-{time}.vcboard"));
            let mut doc = self.doc.clone();
            for obj in &mut doc.objects {
                if let Some(source) = obj.content.source_path_mut() {
                    *source = resolve(source, &self.path);
                }
            }
            if let Err(error) = write(&target, &doc) {
                eprintln!("Board recovery failed: {error}");
            }
        }
    }
}
fn open_shared(path: &Path) -> Result<Arc<Mutex<Shared>>> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<Shared>>>>> = OnceLock::new();
    let path = std::fs::canonicalize(path)?;
    let mut open = OPEN.get_or_init(Default::default).lock().unwrap();
    open.retain(|_, v| v.strong_count() > 0);
    if let Some(existing) = open.get(&path).and_then(Weak::upgrade) {
        return Ok(existing);
    }
    let shared = Arc::new(Mutex::new(Shared {
        doc: read(&path)?,
        saved: stamp(&path),
        path: path.clone(),
        undo: vec![],
        redo: vec![],
        dirty: false,
        due: Instant::now(),
        error: None,
        gesture: None,
    }));
    open.insert(path, Arc::downgrade(&shared));
    Ok(shared)
}

pub fn thumbnail(path: &Path) -> Result<egui::ColorImage> {
    let doc = read(path)?;
    // A schematic cover does not recursively decode every image in a board while browsing.
    let mut out = egui::ColorImage::new([320, 240], Color32::from_rgb(19, 24, 31));
    let bounds = doc
        .objects
        .iter()
        .map(Object::bounds)
        .reduce(Rect::union)
        .unwrap_or(Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0)));
    let scale = (280.0 / bounds.width().max(1.0)).min(200.0 / bounds.height().max(1.0));
    for o in &doc.objects {
        let r = o.bounds();
        let center = Pos2::new(160.0, 120.0) + (r.center() - bounds.center()) * scale;
        let size = r.size() * scale;
        let area = Rect::from_center_size(center, size);
        let color = match o.content {
            Content::Image { .. } | Content::ArchiveImage { .. } => Color32::from_rgb(89, 155, 192),
            Content::Video { .. } => Color32::from_rgb(120, 180, 155),
            Content::Markdown { .. } => Color32::from_rgb(150, 165, 210),
            Content::Text { .. } => Color32::from_rgb(219, 183, 110),
            Content::Ink { .. } => Color32::from_rgb(162, 124, 195),
        };
        for y in (area.top().max(0.0) as usize)..(area.bottom().min(240.0) as usize) {
            for x in (area.left().max(0.0) as usize)..(area.right().min(320.0) as usize) {
                out[(x, y)] = color;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture(label: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vc-board-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn relative_references_notes_and_ink_survive_relocation_without_touching_sources() {
        let root = fixture("roundtrip");
        let file = root.join("scene.vcboard");
        let image = root.join("reference.png");
        image::RgbaImage::from_pixel(20, 40, image::Rgba([80, 140, 190, 255]))
            .save(&image)
            .unwrap();
        let original = std::fs::read(&image).unwrap();
        let mut doc = Document::new("Свет / Lighting".into());
        doc.add_images(&[image.clone()], &file, Pos2::ZERO);
        doc.objects[0].angle = 0.75;
        doc.objects.push(Object {
            id: 2,
            center: [80.0, 45.0],
            size: [180.0, 90.0],
            angle: 0.2,
            options: Default::default(),
            content: Content::Text {
                text: "Заметка\nLine two".into(),
                color: [255, 200, 100, 255],
            },
        });
        doc.objects.push(Object {
            id: 3,
            center: [50.0, 10.0],
            size: [20.0, 30.0],
            angle: 0.0,
            options: Default::default(),
            content: Content::Ink {
                points: vec![[-0.5, -0.5], [0.5, 0.5]],
                color: [255; 4],
                width: 2.0,
            },
        });
        write(&file, &doc).unwrap();
        assert_eq!(read(&file).unwrap(), doc);
        if let Content::Image { source } = &doc.objects[0].content {
            assert_eq!(source, Path::new("reference.png"));
        } else {
            panic!();
        }
        let moved = root.join("moved");
        std::fs::create_dir(&moved).unwrap();
        std::fs::copy(&image, moved.join("reference.png")).unwrap();
        std::fs::copy(&file, moved.join("scene.vcboard")).unwrap();
        let restored = read(&moved.join("scene.vcboard")).unwrap();
        if let Content::Image { source } = &restored.objects[0].content {
            assert!(resolve(source, &moved.join("scene.vcboard")).exists());
        }
        assert_eq!(std::fs::read(&image).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_board_is_rejected_and_external_changes_are_not_overwritten() {
        let root = fixture("conflict");
        let file = root.join("board.vcboard");
        let doc = Document::new("Original".into());
        write(&file, &doc).unwrap();
        let first = open_shared(&file).unwrap();
        let second = open_shared(&file).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        {
            let mut s = first.lock().unwrap();
            s.doc.title = "Our edit".into();
            s.changed();
        }
        let mut outside = doc.clone();
        outside.title = "External modification to preserve".into();
        write(&file, &outside).unwrap();
        {
            let mut s = first.lock().unwrap();
            assert!(s.flush().is_err());
            assert_eq!(read(&file).unwrap().title, outside.title);
            s.dirty = false;
        }
        let mut invalid = doc.clone();
        invalid.version = 999;
        assert!(invalid.validate().is_err());
        invalid = doc;
        invalid.camera.zoom = f32::NAN;
        assert!(invalid.validate().is_err());
        drop(first);
        drop(second);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn zoom_anchor_and_rotated_hit_test_are_consistent() {
        let area = Rect::from_min_size(Pos2::new(200.0, 100.0), Vec2::new(800.0, 500.0));
        let mut camera = Camera::default();
        let point = Pos2::new(721.0, 231.0);
        let world = camera.world(point, area);
        camera.zoom_at(point, area, 3.2);
        assert!(camera.world(point, area).distance(world) < 0.0001);
        let object = Object {
            id: 1,
            center: [10.0, 20.0],
            size: [100.0, 20.0],
            angle: std::f32::consts::FRAC_PI_2,
            options: Default::default(),
            content: Content::Text {
                text: "Note".into(),
                color: [255; 4],
            },
        };
        assert!(object.hit(Pos2::new(10.0, 60.0), 0.0));
        assert!(!object.hit(Pos2::new(50.0, 20.0), 0.0));
    }
    #[test]
    fn selecting_board_in_browser_and_delete_on_canvas_keeps_original_files() {
        let root = fixture("browser");
        let path = root.join("board.vcboard");
        let image = root.join("image.png");
        image::RgbImage::new(8, 8).save(&image).unwrap();
        let mut doc = Document::new("Board".into());
        doc.add_images(&[image.clone()], &path, Pos2::ZERO);
        write(&path, &doc).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.open_path(&path, &ctx);
        assert!(app.board.is_some());
        assert!(app.texture.is_none());
        app.board.as_mut().unwrap().focused = true;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                app.file_commands(ctx);
            },
        );
        assert!(app.workbench.recycle.is_none());
        assert!(path.exists() && image.exists());
        app.new_tab(false, &ctx);
        assert!(app.board.is_none());
        app.switch_tab(0);
        assert!(app.board.is_some());
        app.select(Node::Computer, &ctx);
        assert!(app.board.is_none());
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
