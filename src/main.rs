//! view-calipers: Windows media browser with dual panes, tabs, and split preview.
//! See README.md for formats, requirements, controls, and limitations.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::cmp::Ordering;
mod video;
mod model3d;
mod settings;
use settings::Action;
mod timeline;
mod documents;
mod thumbnail_cache;
mod image_decode;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;
use std::sync::{mpsc, Arc, Mutex, atomic::{AtomicU64, Ordering as AtomicOrdering}};

use anyhow::{anyhow, Context, Result};
use eframe::egui;
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, RichText, Sense, TextureHandle, TextureOptions, Vec2};

const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "tga", "exr", "hdr", "ico", "pnm",
    "pbm", "pgm", "ppm", "qoi", "dds",
];
const VIDEO_EXT: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "m4v", "wmv", "mpg", "mpeg", "ts", "mts", "m2ts", "flv", "ogv"];
const DOCUMENT_EXT: &[&str] = &["pdf", "md", "markdown"];
#[cfg(feature = "sevenz")]
const ARCHIVE_EXT: &[&str] = &["zip", "cbz", "7z", "cb7", "rar", "cbr"];
#[cfg(not(feature = "sevenz"))]
const ARCHIVE_EXT: &[&str] = &["zip", "cbz", "rar", "cbr"];

/// Картинки крупнее этого ужимаются перед загрузкой в GPU-текстуру.
const MAX_TEX: u32 = 8192;

// ───────────────────────────── helpers ─────────────────────────────

fn ext_of_path(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn ext_of_name(n: &str) -> String {
    match n.rfind('.') {
        Some(i) => n[i + 1..].to_ascii_lowercase(),
        None => String::new(),
    }
}

fn is_image_ext(e: &str) -> bool {
    IMAGE_EXT.contains(&e)
}

fn is_archive_ext(e: &str) -> bool {
    ARCHIVE_EXT.contains(&e)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn take_digits(it: &mut Peekable<Chars>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek() {
        if c.is_ascii_digit() {
            s.push(*c);
            it.next();
        } else {
            break;
        }
    }
    s
}

/// Естественная сортировка: img2 < img10.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_digits(&mut ai);
                    let nb = take_digits(&mut bi);
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    ai.next();
                    bi.next();
                    let ord = ca.cmp(&cb);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
}

// ───────────────────────────── archives ─────────────────────────────

fn index_zip(path: &Path) -> Result<Vec<String>> {
    let f = File::open(path)?;
    let z = zip::ZipArchive::new(f)?;
    Ok(z.file_names()
        .filter(|n| !n.ends_with('/'))
        .map(|n| n.replace('\\', "/"))
        .collect())
}

fn read_zip(path: &Path, name: &str) -> Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    let index = z.file_names()
        .position(|entry| entry.replace('\\', "/") == name)
        .ok_or_else(|| anyhow!("entry not found: {name}"))?;
    let mut e = z.by_index(index)?;
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(feature = "sevenz")]
fn index_7z(path: &Path) -> Result<Vec<String>> {
    let r = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    Ok(r.archive()
        .files
        .iter()
        .filter(|e| !e.is_directory())
        .map(|e| e.name().replace('\\', "/"))
        .collect())
}

#[cfg(feature = "sevenz")]
fn read_7z(path: &Path, name: &str) -> Result<Vec<u8>> {
    let mut r = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    let mut out: Option<Vec<u8>> = None;
    let mut io_err: Option<std::io::Error> = None;
    r.for_each_entries(|entry, reader| {
        if entry.name().replace('\\', "/") == name {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            match reader.read_to_end(&mut buf) {
                Ok(_) => out = Some(buf),
                Err(e) => io_err = Some(e),
            }
            return Ok(false); // стоп
        }
        Ok(true)
    })?;
    if let Some(e) = io_err {
        return Err(e.into());
    }
    out.ok_or_else(|| anyhow!("entry not found: {name}"))
}

fn index_archive(path: &Path) -> Result<Vec<String>> {
    match ext_of_path(path).as_str() {
        "rar" | "cbr" => {
            let mut entries = Vec::new();
            for entry in unrar::Archive::new(path).open_for_listing()? {
                let entry = entry?;
                if !entry.is_directory() { entries.push(entry.filename.to_string_lossy().replace('\\', "/")); }
            }
            Ok(entries)
        }
        "zip" | "cbz" => index_zip(path),
        #[cfg(feature = "sevenz")]
        "7z" | "cb7" => index_7z(path),
        e => Err(anyhow!("unsupported archive: .{e}")),
    }
}

fn read_archive_entry(path: &Path, name: &str) -> Result<Vec<u8>> {
    match ext_of_path(path).as_str() {
        "rar" | "cbr" => {
            let mut archive = unrar::Archive::new(path).open_for_processing()?;
            while let Some(entry) = archive.read_header()? {
                if entry.entry().filename.to_string_lossy().replace('\\', "/") == name {
                    return Ok(entry.read()?.0);
                }
                archive = entry.skip()?;
            }
            Err(anyhow!("entry not found: {name}"))
        }
        "zip" | "cbz" => read_zip(path, name),
        #[cfg(feature = "sevenz")]
        "7z" | "cb7" => read_7z(path, name),
        e => Err(anyhow!("unsupported archive: .{e}")),
    }
}

// ───────────────────────────── model ─────────────────────────────

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Node {
    Computer,
    Dir(PathBuf),
    Archive(PathBuf),
    /// Виртуальная папка внутри архива. `prefix` пустой или заканчивается на '/'.
    ArchiveDir { archive: PathBuf, prefix: String },
}

impl Node {
    fn breadcrumbs(&self) -> Vec<Node> {
        let mut nodes = vec![Node::Computer];
        let (path, archive_prefix) = match self {
            Node::Computer => return nodes,
            Node::Dir(path) => (path, None),
            Node::Archive(path) => (path, Some("")),
            Node::ArchiveDir { archive, prefix } => (archive, Some(prefix.as_str())),
        };
        let folder = if archive_prefix.is_some() { path.parent() } else { Some(path.as_path()) };
        if let Some(folder) = folder {
            nodes.extend(folder.ancestors().filter(|p| !p.as_os_str().is_empty())
                .collect::<Vec<_>>().into_iter().rev().map(|p| Node::Dir(p.to_path_buf())));
        }
        if let Some(prefix) = archive_prefix {
            nodes.push(Node::Archive(path.clone()));
            let mut current = String::new();
            for part in prefix.split('/').filter(|part| !part.is_empty()) {
                current.push_str(part); current.push('/');
                nodes.push(Node::ArchiveDir { archive: path.clone(), prefix: current.clone() });
            }
        }
        nodes
    }
    fn label(&self) -> String {
        match self {
            Node::Computer => "This PC".into(),
            Node::Dir(p) => file_name(p),
            Node::Archive(p) => file_name(p),
            Node::ArchiveDir { prefix, .. } => prefix
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(prefix)
                .to_string(),
        }
    }
    fn is_archive_like(&self) -> bool {
        matches!(self, Node::Archive(_) | Node::ArchiveDir { .. })
    }
}

fn computer_drives() -> Vec<Node> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" { fn GetLogicalDrives() -> u32; }
        // The bit mask includes mounted local, removable and mapped network drives.
        let mask = unsafe { GetLogicalDrives() };
        (0..26).filter(|i| mask & (1 << i) != 0)
            .map(|i| Node::Dir(PathBuf::from(format!("{}:\\", (b'A' + i) as char))))
            .collect()
    }
    #[cfg(not(windows))]
    { vec![Node::Dir(PathBuf::from("/"))] }
}

fn bookmarks_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join("ArchView").join("bookmarks.txt")
}

fn read_bookmarks(path: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(path).unwrap_or_default().lines()
        .filter(|line| !line.is_empty()).map(PathBuf::from).collect()
}

fn write_bookmarks(path: &Path, bookmarks: &[PathBuf]) -> Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let text = bookmarks.iter().map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>().join("\n");
    let temporary = path.with_extension("txt.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[derive(Clone, Debug)]
enum ImageSrc {
    File(PathBuf),
    Entry { archive: PathBuf, name: String },
}

fn folder_cover(node: &Node) -> Option<ImageSrc> {
    match node {
        Node::Computer => None,
        Node::Dir(path) => {
            let mut images: Vec<PathBuf> = std::fs::read_dir(path).ok()?.flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_image_ext(&ext_of_path(path))).collect();
            images.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
            images.into_iter().next().map(ImageSrc::File)
        }
        Node::Archive(archive) | Node::ArchiveDir { archive, .. } => {
            let prefix = match node { Node::ArchiveDir { prefix, .. } => prefix.as_str(), _ => "" };
            let mut entries: Vec<String> = index_archive(archive).ok()?.into_iter()
                .filter(|name| name.starts_with(prefix) && is_image_ext(&ext_of_name(name))).collect();
            entries.sort_by(|a, b| natural_cmp(a, b));
            entries.into_iter().next().map(|name| ImageSrc::Entry { archive: archive.clone(), name })
        }
    }
}

fn decode_thumbnail(source: &ImageSrc, use_cache: bool) -> Result<egui::ColorImage> {
                    let epoch = thumbnail_cache::epoch();
                    let cached_path = if use_cache { thumbnail_cache::key(source, &thumbnail_cache::directory()).ok() } else { None };
                    if let Some(rgba) = cached_path.as_ref().and_then(|path| thumbnail_cache::load(path)) {
                        return Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()));
                    }
                    let bytes = match source {
                        ImageSrc::File(path) if model3d::EXTENSIONS.contains(&ext_of_path(path).as_str()) => {
                            let model = model3d::load(path)?;
                            let color = model3d::thumbnail(&model);
                            if let Some(path) = &cached_path {
                                let mut rgba = image::RgbaImage::new(color.size[0] as u32, color.size[1] as u32);
                                for (pixel, color) in rgba.pixels_mut().zip(&color.pixels) { *pixel = image::Rgba(color.to_array()); }
                                let _ = thumbnail_cache::save(path, &rgba, epoch);
                            }
                            return Ok(color);
                        }
                        ImageSrc::File(path) if ext_of_path(path) == "pdf" => documents::pdf_page(path, 0, 320)?.1,
                        ImageSrc::File(path) if ["md", "markdown"].contains(&ext_of_path(path).as_str()) => {
                            let rgba = documents::markdown_thumbnail(&std::fs::read_to_string(path)?);
                            if let Some(path) = &cached_path { let _ = thumbnail_cache::save(path, &rgba, epoch); }
                            return Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()));
                        }
                        ImageSrc::File(path) if VIDEO_EXT.contains(&ext_of_path(path).as_str()) => video::poster(path, 0.0, 320, 240)?,
                        ImageSrc::File(path) => std::fs::read(path)?,
                        ImageSrc::Entry { archive, name } => read_archive_entry(archive, name)?,
                    };
                    let decoded = image_decode::decode(&bytes)?;
                    let rgba = decoded.thumbnail(320.min(decoded.width()), 240.min(decoded.height())).to_rgba8();
                    if let Some(path) = &cached_path { let _ = thumbnail_cache::save(path, &rgba, epoch); }
                    Ok(egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

#[derive(Clone, Debug)]
struct ImageRef {
    label: String,
    src: ImageSrc,
}

struct BrowserTab {
    locked: bool,
    title: String,
    state: Option<Box<App>>,
}

struct App {
    tabs: Vec<BrowserTab>,
    active_tab: usize,
    split_preview: bool,
    preview_fraction: f32,
    retired_renderers: Vec<model3d::Renderer>,
    settings: settings::Settings,
    settings_open: bool,
    key_capture: Option<Action>,
    settings_message: String,
    settings_initialized: bool,
    dual_panes: bool,
    active_pane: usize,
    pane_fraction: f32,
    inactive: Option<Box<App>>,
    history: Vec<Node>,
    history_index: usize,
    model: Option<Arc<model3d::Model>>,
    model_loader: Option<model3d::Loader>,
    model_request: Option<u64>,
    model_error: Option<String>,
    model_view: model3d::View,
    model_renderer: model3d::Renderer,
    pdf: Option<(PathBuf, u32, u32)>,
    pdf_scrubber: Option<documents::PdfScrubber>,
    pdf_request: Option<(u32, u32)>,
    markdown: Option<String>,
    markdown_cache: egui_commonmark::CommonMarkCache,
    cache_thumbnails: bool,
    fullscreen: bool,
    video_seek: Option<f64>,
    scrubber: Option<video::Scrubber>,
    scrub_resume: bool,
    thumb_queue: Option<Arc<Mutex<VecDeque<usize>>>>,
    video: Option<video::Player>,
    thumb_size: f32,
    archive_errors: HashMap<PathBuf, String>,
    bookmarks: Vec<PathBuf>,
    gallery: bool,
    gallery_nodes: Vec<Node>,
    thumbs: Vec<Option<TextureHandle>>,
    thumb_errors: HashMap<usize, String>,
    thumb_rx: Option<mpsc::Receiver<(usize, Result<egui::ColorImage>)>>,
    thumb_generation: Arc<AtomicU64>,
    expanded: HashSet<Node>,
    selected: Option<Node>,
    /// Кэш списков файлов внутри архивов.
    archive_cache: HashMap<PathBuf, Vec<String>>,
    images: Vec<ImageRef>,
    current: Option<usize>,
    texture: Option<TextureHandle>,
    tex_size: [usize; 2],
    zoom: f32,
    fit: bool,
    pan: Vec2,
    status: String,
    pending_open: Option<PathBuf>,
}

impl App {
    fn new(start: Option<PathBuf>) -> Self {
        let mut app = Self {
            tabs: vec![BrowserTab { locked: false, title: "This PC".into(), state: None }],
            active_tab: 0,
            split_preview: false,
            preview_fraction: 0.5,
            retired_renderers: Vec::new(),
            settings: settings::Settings::default(),
            settings_open: false,
            key_capture: None,
            settings_message: String::new(),
            settings_initialized: false,
            dual_panes: false,
            active_pane: 0,
            pane_fraction: 0.5,
            inactive: None,
            history: vec![Node::Computer],
            history_index: 0,
            model: None,
            model_loader: None,
            model_request: None,
            model_error: None,
            model_view: model3d::View::default(),
            model_renderer: Arc::new(Mutex::new(None)),
            pdf: None,
            pdf_scrubber: None,
            pdf_request: None,
            markdown: None,
            markdown_cache: egui_commonmark::CommonMarkCache::default(),
            cache_thumbnails: true,
            fullscreen: false,
            video_seek: None,
            scrubber: None,
            scrub_resume: false,
            thumb_queue: None,
            video: None,
            thumb_size: 160.0,
            archive_errors: HashMap::new(),
            bookmarks: read_bookmarks(&bookmarks_path()),
            gallery: true,
            gallery_nodes: Vec::new(),
            thumbs: Vec::new(),
            thumb_errors: HashMap::new(),
            thumb_rx: None,
            thumb_generation: Arc::new(AtomicU64::new(0)),
            expanded: HashSet::new(),
            selected: None,
            archive_cache: HashMap::new(),
            images: Vec::new(),
            current: None,
            texture: None,
            tex_size: [0, 0],
            zoom: 1.0,
            fit: true,
            pan: Vec2::ZERO,
            status: String::new(),
            pending_open: None,
        };
        app.expanded.insert(Node::Computer);
        app.selected = Some(Node::Computer);
        app.gallery_nodes = computer_drives();
        app.thumbs = vec![None; app.gallery_nodes.len()];
        app.status = format!("This PC — {} drives", app.gallery_nodes.len());
        app.pending_open = start;
        app
    }

    // ── tree data ──

    fn archive_entries(&mut self, archive: &Path) -> Vec<String> {
        if self.archive_errors.contains_key(archive) { return Vec::new(); }
        if let Some(v) = self.archive_cache.get(archive) {
            return v.clone();
        }
        let entries = match index_archive(archive) {
            Ok(v) => v,
            Err(e) => {
                let message = format!("Could not read archive {}: {e}", file_name(archive));
                self.status = message.clone();
                self.archive_errors.insert(archive.to_path_buf(), message);
                return Vec::new();
            }
        };
        self.archive_cache.insert(archive.to_path_buf(), entries.clone());
        entries
    }

    fn children(&mut self, node: &Node) -> Vec<Node> {
        match node {
            Node::Computer => computer_drives(),
            Node::Dir(p) => {
                let mut dirs: Vec<PathBuf> = Vec::new();
                let mut archives: Vec<PathBuf> = Vec::new();
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        let path = e.path();
                        let name = file_name(&path);
                        if name.starts_with('.') {
                            continue;
                        }
                        if path.is_dir() {
                            dirs.push(path);
                        } else if is_archive_ext(&ext_of_path(&path)) {
                            archives.push(path);
                        }
                    }
                }
                dirs.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
                archives.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
                dirs.into_iter()
                    .map(Node::Dir)
                    .chain(archives.into_iter().map(Node::Archive))
                    .collect()
            }
            Node::Archive(p) => self.archive_subdirs(p, ""),
            Node::ArchiveDir { archive, prefix } => self.archive_subdirs(archive, prefix),
        }
    }

    fn archive_subdirs(&mut self, archive: &Path, prefix: &str) -> Vec<Node> {
        let entries = self.archive_entries(archive);
        let mut subs: BTreeSet<String> = BTreeSet::new();
        for n in &entries {
            if let Some(rest) = n.strip_prefix(prefix) {
                if let Some(i) = rest.find('/') {
                    subs.insert(rest[..i].to_string());
                }
            }
        }
        let mut subs: Vec<String> = subs.into_iter().collect();
        subs.sort_by(|a, b| natural_cmp(a, b));
        subs.into_iter()
            .map(|s| Node::ArchiveDir {
                archive: archive.to_path_buf(),
                prefix: format!("{prefix}{s}/"),
            })
            .collect()
    }

    fn images_of(&mut self, node: &Node) -> Vec<ImageRef> {
        match node {
            Node::Computer => Vec::new(),
            Node::Dir(p) => {
                let mut v: Vec<PathBuf> = std::fs::read_dir(p)
                    .map(|rd| {
                        rd.flatten()
                            .map(|e| e.path())
                            .filter(|p| p.is_file() && (is_image_ext(&ext_of_path(p)) || VIDEO_EXT.contains(&ext_of_path(p).as_str()) || DOCUMENT_EXT.contains(&ext_of_path(p).as_str()) || model3d::EXTENSIONS.contains(&ext_of_path(p).as_str())))
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
                v.into_iter()
                    .map(|p| ImageRef { label: file_name(&p), src: ImageSrc::File(p) })
                    .collect()
            }
            Node::Archive(p) => self.archive_images(p, ""),
            Node::ArchiveDir { archive, prefix } => self.archive_images(archive, prefix),
        }
    }

    fn archive_images(&mut self, archive: &Path, prefix: &str) -> Vec<ImageRef> {
        let entries = self.archive_entries(archive);
        let mut v: Vec<String> = entries
            .into_iter()
            .filter(|n| {
                n.strip_prefix(prefix)
                    .map(|rest| is_image_ext(&ext_of_name(rest)))
                    .unwrap_or(false)
            })
            .collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v.into_iter()
            .map(|n| ImageRef {
                label: n[prefix.len()..].to_string(),
                src: ImageSrc::Entry { archive: archive.to_path_buf(), name: n },
            })
            .collect()
    }

    // ── navigation ──

    fn open_path(&mut self, p: &Path, ctx: &egui::Context) {
        if p.is_dir() {
            let n = Node::Dir(p.to_path_buf());
            self.expanded.insert(n.clone());
            self.select(n, ctx);
        } else if p.is_file() {
            let parent = p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
            self.expanded.insert(Node::Dir(parent.clone()));
            let ext = ext_of_path(p);
            if is_archive_ext(&ext) {
                let n = Node::Archive(p.to_path_buf());
                self.expanded.insert(n.clone());
                self.select(n, ctx);
            } else {
                self.select(Node::Dir(parent), ctx);
                if let Some(i) = self.images.iter().position(|im| {
                    matches!(&im.src, ImageSrc::File(fp) if fp == p)
                }) {
                    self.goto(i, ctx);
                }
            }
        }
    }

    fn select(&mut self, node: Node, ctx: &egui::Context) {
        if self.tabs[self.active_tab].locked && self.selected.as_ref() != Some(&node) {
            self.new_tab(false, ctx);
        }
        self.tabs[self.active_tab].title = node.label();
        if self.history.get(self.history_index) != Some(&node) {
            self.history.truncate(self.history_index + 1);
            self.history.push(node.clone());
            self.history_index = self.history.len() - 1;
        }
        self.model = None;
        self.model_request = None;
        self.model_error = None;
        self.pdf = None;
        self.pdf_scrubber = None;
        self.pdf_request = None;
        self.markdown = None;
        self.video_seek = None;
        self.scrubber = None;
        self.scrub_resume = false;
        self.video = None;
        self.reveal_ancestors(&node);
        match &node {
            Node::Archive(path) | Node::ArchiveDir { archive: path, .. } => {
                self.archive_errors.remove(path);
            }
            _ => {}
        }
        self.images = self.images_of(&node);
        self.gallery_nodes = self.children(&node);
        self.selected = Some(node);
        self.current = None;
        self.texture = None;
        self.gallery = true;
        self.status = format!("{} folders/archives, {} media files", self.gallery_nodes.len(), self.images.len());
        if let Some(Node::Archive(path) | Node::ArchiveDir { archive: path, .. }) = &self.selected {
            if let Some(error) = self.archive_errors.get(path) { self.status = error.clone(); }
        }
        self.start_thumbnails(ctx);
    }

    fn reveal_ancestors(&mut self, node: &Node) {
        self.expanded.insert(Node::Computer);
        let path = match node {
            Node::Computer => return,
            Node::Dir(path) | Node::Archive(path) => path,
            Node::ArchiveDir { archive, prefix } => {
                self.expanded.insert(Node::Archive(archive.clone()));
                let mut parent = prefix.trim_end_matches('/');
                while let Some((ancestor, _)) = parent.rsplit_once('/') {
                    self.expanded.insert(Node::ArchiveDir {
                        archive: archive.clone(), prefix: format!("{ancestor}/"),
                    });
                    parent = ancestor;
                }
                archive
            }
        };
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() { break; }
            self.expanded.insert(Node::Dir(ancestor.to_path_buf()));
        }
    }

    fn goto(&mut self, i: usize, ctx: &egui::Context) {
        if i >= self.images.len() {
            return;
        }
        self.current = Some(i);
        self.model = None;
        self.model_request = None;
        self.model_error = None;
        self.pdf = None;
        self.pdf_scrubber = None;
        self.pdf_request = None;
        self.markdown = None;
        self.texture = None;
        self.video_seek = None;
        self.scrubber = None;
        self.scrub_resume = false;
        self.video = None;
        self.gallery = true;
        if let ImageSrc::File(path) = &self.images[i].src {
            if model3d::EXTENSIONS.contains(&ext_of_path(path).as_str()) {
                self.model_view = model3d::View::default();
                self.model_request = Some(self.model_loader.get_or_insert_with(|| model3d::Loader::new(ctx.clone())).request(path.clone()));
                self.status = format!("Loading 3D model: {}", self.images[i].label);
                return;
            }
        }
        match self.load_image(ctx, &self.images[i].src.clone()) {
            Ok(()) => {
                self.fit = true;
                self.pan = Vec2::ZERO;
                self.status = format!("{} / {}  —  {}", i + 1, self.images.len(), self.images[i].label);
            }
            Err(e) => {
                self.texture = None;
                self.status = format!("Error: {e}");
            }
        }
    }

    fn step(&mut self, d: isize, ctx: &egui::Context) {
        if let Some((_, page, count)) = &self.pdf {
            let next = (*page as isize + d).clamp(0, *count as isize - 1) as u32;
            self.load_pdf_page(next, ctx);
            return;
        }
        if self.images.is_empty() {
            return;
        }
        let cur = self.current.unwrap_or(0) as isize;
        let n = self.images.len() as isize;
        let next = (cur + d).rem_euclid(n) as usize;
        self.goto(next, ctx);
    }

    fn load_image(&mut self, ctx: &egui::Context, src: &ImageSrc) -> Result<()> {
        let bytes = match src {
            ImageSrc::File(path) if ext_of_path(path) == "pdf" => {
                let (count, bytes) = documents::pdf_page(path, 0, 1600)?;
                self.pdf = Some((path.clone(), 0, count));
                bytes
            }
            ImageSrc::File(path) if ["md", "markdown"].contains(&ext_of_path(path).as_str()) => {
                self.markdown = Some(std::fs::read_to_string(path)?.trim_start_matches('\u{feff}').to_string());
                self.markdown_cache = egui_commonmark::CommonMarkCache::default();
                return Ok(());
            }
            ImageSrc::File(p) if VIDEO_EXT.contains(&ext_of_path(p).as_str()) => {
                let bytes = video::poster(p, 0.0, video::WIDTH as u32, video::HEIGHT as u32)?;
                let poster = image::load_from_memory(&bytes)?;
                let player = video::Player::new(p.clone(), [poster.width() as usize, poster.height() as usize])?;
                self.video = Some(player);
                bytes
            }
            ImageSrc::File(p) => std::fs::read(p).with_context(|| p.display().to_string())?,
            ImageSrc::Entry { archive, name } => read_archive_entry(archive, name)?,
        };
        let mut img = image_decode::decode(&bytes).context("decode")?;
        if img.width() > MAX_TEX || img.height() > MAX_TEX {
            img = img.thumbnail(MAX_TEX, MAX_TEX);
        }
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        self.texture = Some(ctx.load_texture("current", color, TextureOptions::LINEAR));
        self.tex_size = size;
        Ok(())
    }

    // ── UI ──

    fn request_pdf_page(&mut self, page: u32, width: u32, ctx: &egui::Context) {
        let Some((path, current, count)) = self.pdf.clone() else { return; };
        let page = page.min(count.saturating_sub(1));
        if self.pdf_request == Some((page, width)) { return; }
        self.pdf_scrubber.get_or_insert_with(|| documents::PdfScrubber::new(path.clone(), ctx.clone())).seek(page, width);
        self.pdf_request = Some((page, width));
        self.pdf = Some((path, page, count));
        if page != current { self.fit = true; self.pan = Vec2::ZERO; }
    }

    fn load_pdf_page(&mut self, page: u32, ctx: &egui::Context) { self.request_pdf_page(page, 1600, ctx); }

    fn poll_pdf_page(&mut self, ctx: &egui::Context) {
        if let Some((page, width, result)) = self.pdf_scrubber.as_ref().and_then(documents::PdfScrubber::poll) {
            if self.pdf_request != Some((page, width)) { return; }
            match result {
                Ok(color) => {
                    self.tex_size = color.size;
                    self.texture = Some(ctx.load_texture("pdf-page", color, TextureOptions::LINEAR));
                }
                Err(error) => { self.pdf_request = None; self.status = format!("PDF: {error}"); }
            }
        }
    }

    fn show_document_controls(&mut self, ui: &mut egui::Ui) {
        if let Some((_, page, count)) = self.pdf.clone() {
            let mut target = page + 1;
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(page > 0, egui::Button::new("← Page")).clicked() { target = page; }
                ui.add(egui::DragValue::new(&mut target).range(1..=count));
                ui.label(format!("/ {count}"));
                if ui.add_enabled(page + 1 < count, egui::Button::new("Page →")).clicked() { target = page + 2; }
            });
            if target != page + 1 { self.load_pdf_page(target - 1, ui.ctx()); }
            let current = self.pdf.as_ref().map(|(_, page, _)| page + 1).unwrap_or(target);
            let mut position = current;
            let old_width = ui.spacing().slider_width;
            ui.spacing_mut().slider_width = ui.available_width().max(1.0);
            let response = ui.add_enabled(count > 1, egui::Slider::new(&mut position, 1..=count)
                .show_value(false).text("")).on_hover_text("Drag to scrub through PDF pages");
            ui.spacing_mut().slider_width = old_width;
            if response.changed() { self.request_pdf_page(position - 1, if response.dragged() { 900 } else { 1600 }, ui.ctx()); }
            if response.drag_stopped() { self.request_pdf_page(position - 1, 1600, ui.ctx()); }
            ui.horizontal(|ui| {
                ui.label("1");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(count.to_string()); });
            });
        }
    }
    fn set_fullscreen(&mut self, ctx: &egui::Context, fullscreen: bool) {
        self.fullscreen = fullscreen;
        if self.model.is_none() { self.fit = true; }
        self.pan = Vec2::ZERO;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
    }

    fn add_bookmark(&mut self, path: PathBuf) {
        if self.bookmarks.iter().any(|p| p.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase()) {
            return;
        }
        self.bookmarks.push(path);
        if let Err(error) = write_bookmarks(&bookmarks_path(), &self.bookmarks) {
            self.status = format!("Could not save bookmarks: {error}");
        }
    }

    fn show_bookmarks(&mut self, ui: &mut egui::Ui) -> Option<Node> {
        let selected_folder = match &self.selected {
            Some(Node::Dir(path)) => Some(path.clone()),
            _ => None,
        };
        ui.horizontal(|ui| {
            ui.strong("Folder bookmarks");
            if ui.add_enabled(selected_folder.is_some(), egui::Button::new("+ Add"))
                .on_hover_text("Bookmark the selected folder").clicked() {
                if let Some(path) = selected_folder { self.add_bookmark(path); }
            }
        });
        ui.separator();
        let mut clicked = None;
        let mut remove = None;
        egui::ScrollArea::both().id_salt("bookmarks_scroll").auto_shrink([false, false]).show(ui, |ui| {
            if self.bookmarks.is_empty() { ui.label("Select a folder and click + Add"); }
            for (i, path) in self.bookmarks.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("×").on_hover_text("Remove bookmark").clicked() { remove = Some(i); }
                    if ui.selectable_label(self.selected.as_ref() == Some(&Node::Dir(path.clone())), file_name(path))
                        .on_hover_text(path.display().to_string()).clicked() {
                        if path.is_dir() { clicked = Some(Node::Dir(path.clone())); }
                        else { self.status = format!("Folder unavailable: {}", path.display()); }
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.bookmarks.remove(i);
            if let Err(error) = write_bookmarks(&bookmarks_path(), &self.bookmarks) {
                self.status = format!("Could not save bookmarks: {error}");
            }
        }
        clicked
    }

    fn start_thumbnails(&mut self, ctx: &egui::Context) {
        self.thumbs = vec![None; self.images.len() + self.gallery_nodes.len()];
        self.thumb_errors.clear();
        let generation = self.thumb_generation.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let sources = Arc::new(self.images.clone());
        let folders = Arc::new(self.gallery_nodes.clone());
        let queue = Arc::new(Mutex::new((0..self.thumbs.len()).collect::<VecDeque<_>>()));
        self.thumb_queue = Some(queue.clone());
        let (tx, rx) = mpsc::sync_channel(16);
        self.thumb_rx = Some(rx);
        for _ in 0..2 {
            let sources = sources.clone(); let folders = folders.clone(); let queue = queue.clone();
            let token = self.thumb_generation.clone(); let tx = tx.clone(); let ctx = ctx.clone();
            let use_cache = self.cache_thumbnails;
            std::thread::spawn(move || loop {
                if token.load(AtomicOrdering::Relaxed) != generation { break; }
                let Some(i) = queue.lock().unwrap().pop_front() else { break; };
                let source = if i < sources.len() { Some(sources[i].src.clone()) } else { folder_cover(&folders[i - sources.len()]) };
                let Some(source) = source else { continue; };
                let result = decode_thumbnail(&source, use_cache);
                if token.load(AtomicOrdering::Relaxed) != generation { break; }
                if tx.send((i, result)).is_err() { break; }
                ctx.request_repaint();
            });
        }
    }
    fn receive_thumbnails(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.thumb_rx {
            while let Ok((i, result)) = rx.try_recv() {
                match result {
                    Ok(color) => self.thumbs[i] = Some(ctx.load_texture(
                        format!("thumb-{i}"), color, TextureOptions::LINEAR)),
                    Err(error) => { self.thumb_errors.insert(i, error.to_string()); }
                }
            }
        }
    }

    fn show_gallery(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Thumbnail size");
            ui.add(egui::Slider::new(&mut self.thumb_size, 80.0..=320.0).suffix(" px"));
            if ui.checkbox(&mut self.cache_thumbnails, "Cache thumbnails").on_hover_text(thumbnail_cache::directory().display().to_string()).changed() {
                self.start_thumbnails(ui.ctx());
            }
            if ui.button("Clear cache").clicked() {
                self.thumb_generation.fetch_add(1, AtomicOrdering::Relaxed);
                self.thumb_rx = None;
                match thumbnail_cache::clear() {
                    Ok(count) => self.status = format!("Cache cleared: {count} files"),
                    Err(error) => self.status = format!("Cache: {error}"),
                }
                let enabled = self.cache_thumbnails;
                self.cache_thumbnails = false;
                self.start_thumbnails(ui.ctx());
                self.cache_thumbnails = enabled;
            }
        });
        ui.separator();
        let tile_width = (self.thumb_size + 10.0).min(ui.available_width()).max(1.0);
        let preview_height = (tile_width - 10.0).max(1.0) * 0.75;
        let tile_height = preview_height + 35.0;
        let columns = ((ui.available_width() + ui.spacing().item_spacing.x)
            / (tile_width + ui.spacing().item_spacing.x)).floor() as usize;
        let columns = columns.max(1);
        let node_count = self.gallery_nodes.len();
        let count = node_count + self.images.len();
        if count == 0 {
            if let Some(Node::Archive(path) | Node::ArchiveDir { archive: path, .. }) = &self.selected {
                if let Some(message) = self.archive_errors.get(path).cloned() {
                    ui.label(RichText::new(message).color(Color32::LIGHT_RED));
                    ui.label("The archive may be damaged or incompletely downloaded.");
                    if ui.button("Retry reading archive").clicked() {
                        if let Some(node) = self.selected.clone() { self.select(node, ui.ctx()); }
                    }
                    return;
                }
            }
            ui.centered_and_justified(|ui| { ui.label("Folder is empty: no media, folders or archives"); });
            return;
        }
        let mut node_click = None;
        let mut image_click = None;
        let mut open_fullscreen = false;
        egui::ScrollArea::vertical().id_salt("gallery").show_rows(ui, tile_height,
            count.div_ceil(columns), |ui, rows| {
                if let Some(queue) = &self.thumb_queue {
                    let mut queue = queue.lock().unwrap();
                    let visible: Vec<_> = (rows.start * columns..(rows.end * columns).min(count))
                        .map(|i| if i < node_count { self.images.len() + i } else { i - node_count }).collect();
                    for i in visible.into_iter().rev() {
                        if let Some(index) = queue.iter().position(|job| *job == i) { queue.remove(index); queue.push_front(i); }
                    }
                }
                for row in rows {
                    ui.horizontal(|ui| {
                        for index in row * columns..((row + 1) * columns).min(count) {
                            ui.vertical(|ui| {
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(tile_width, tile_height), Sense::click());
                                let painter = ui.painter();
                                let selected = index >= node_count && self.current == Some(index - node_count);
                                painter.rect_filled(rect, 5.0, if selected { Color32::from_rgb(40, 75, 100) } else if response.hovered() { Color32::from_gray(55) } else { Color32::from_gray(32) });
                                let preview = Rect::from_min_size(rect.min + Vec2::new(5.0, 5.0), Vec2::new((tile_width - 10.0).max(1.0), preview_height));
                                let label;
                                if index < node_count {
                                    let node = &self.gallery_nodes[index];
                                    label = node.label();
                                    let kind = if matches!(node, Node::Archive(_)) { "Archive" } else { "Folder" };
                                    if let Some(tex) = &self.thumbs[self.images.len() + index] {
                                        let size = tex.size_vec2();
                                        let scale = (preview.width() / size.x).min(preview.height() / size.y);
                                        painter.image(tex.id(), Rect::from_center_size(preview.center(), size * scale),
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                                        painter.text(preview.right_bottom(), Align2::RIGHT_BOTTOM, kind,
                                            FontId::proportional(12.0), Color32::from_rgb(230, 190, 90));
                                    } else {
                                        painter.text(preview.center(), Align2::CENTER_CENTER, kind,
                                            FontId::proportional(23.0), Color32::from_rgb(230, 190, 90));
                                    }
                                    if response.clicked() { node_click = Some(node.clone()); }
                                } else {
                                    let i = index - node_count;
                                    label = self.images[i].label.clone();
                                    if let Some(tex) = &self.thumbs[i] {
                                        let size = tex.size_vec2();
                                        let scale = (preview.width() / size.x).min(preview.height() / size.y);
                                        painter.image(tex.id(), Rect::from_center_size(preview.center(), size * scale),
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                                    } else {
                                        painter.text(preview.center(), Align2::CENTER_CENTER,
                                            if self.thumb_errors.contains_key(&i) { "Error" } else { "Loading…" },
                                            FontId::proportional(14.0), Color32::GRAY);
                                    }
                                    if response.clicked() { image_click = Some(i); }
                                    if response.double_clicked() {
                                        image_click = Some(i);
                                        open_fullscreen = true;
                                    }
                                    if matches!(&self.images[i].src, ImageSrc::File(path) if VIDEO_EXT.contains(&ext_of_path(path).as_str())) {
                                        painter.text(preview.right_bottom(), Align2::RIGHT_BOTTOM, "▶ Video", FontId::proportional(12.0), Color32::WHITE);
                                    }
                                    if let ImageSrc::File(path) = &self.images[i].src {
                                        let ext = ext_of_path(path);
                                        if DOCUMENT_EXT.contains(&ext.as_str()) || model3d::EXTENSIONS.contains(&ext.as_str()) {
                                            painter.text(preview.right_bottom(), Align2::RIGHT_BOTTOM, ext.to_uppercase(), FontId::proportional(12.0), Color32::from_rgb(70, 140, 220));
                                        }
                                    }
                                }
                                let text_rect = Rect::from_min_size(rect.min + Vec2::new(6.0, preview_height + 10.0), Vec2::new((tile_width - 12.0).max(1.0), 22.0));
                                painter.with_clip_rect(text_rect).text(text_rect.left_center(), Align2::LEFT_CENTER,
                                    &label, FontId::proportional(13.0), Color32::WHITE);
                                response.on_hover_text(label);
                            });
                        }
                    });
                }
            });
        if let Some(node) = node_click {
            self.expanded.insert(node.clone());
            self.select(node, ui.ctx());
        }
        if let Some(i) = image_click {
            if self.current != Some(i) { self.goto(i, ui.ctx()); }
            if open_fullscreen && (self.texture.is_some() || self.markdown.is_some() || self.model_request.is_some() || self.model.is_some()) { self.set_fullscreen(ui.ctx(), true); }
        }
    }

    fn show_tree(&mut self, ui: &mut egui::Ui, node: &Node, depth: usize) -> Option<Node> {
        let mut clicked: Option<Node> = None;
        let expanded = self.expanded.contains(node);
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            let (_, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
            egui::collapsing_header::paint_default_icon(ui, if expanded { 1.0 } else { 0.0 }, &resp);
            if resp.clicked() {
                if expanded {
                    self.expanded.remove(node);
                } else {
                    self.expanded.insert(node.clone());
                }
            }
            let mut text = RichText::new(node.label());
            if node.is_archive_like() {
                text = text.color(Color32::from_rgb(230, 190, 90));
            }
            let is_sel = self.selected.as_ref() == Some(node);
            let response = ui.selectable_label(is_sel, text);
            if response.clicked() {
                clicked = Some(node.clone());
            }
            if let Node::Dir(path) = node {
                response.context_menu(|ui| {
                    if ui.button("Bookmark this folder").clicked() {
                        self.add_bookmark(path.clone());
                        ui.close_menu();
                    }
                });
            }
        });
        if self.expanded.contains(node) {
            for child in self.children(node) {
                if let Some(c) = self.show_tree(ui, &child, depth + 1) {
                    clicked = Some(c);
                }
            }
        }
        clicked
    }

    fn show_video_controls(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
                if let Some(player) = &mut self.video {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(if player.playing() { "Pause" } else { "▶ Play" }).clicked() {
                            if player.playing() { player.pause(); }
                            else if let Err(error) = player.play(&ctx) { self.status = format!("Video: {error}"); }
                        }
                        if ui.checkbox(&mut player.muted, "Mute").changed() && player.playing() {
                            if let Err(error) = player.play(&ctx) { self.status = format!("Video: {error}"); }
                        }
                        ui.checkbox(&mut player.range.repeat, "Loop");
                        let mut gpu = video::GPU.load(AtomicOrdering::Relaxed);
                        if ui.checkbox(&mut gpu, "GPU decoding").on_hover_text("FFmpeg hardware decoding; automatic accelerator selection").changed() {
                            video::GPU.store(gpu, AtomicOrdering::Relaxed);
                            if player.playing() { let _ = player.play(&ctx); }
                        }
                    });
                    let mut range_changed = false;
                    ui.horizontal_wrapped(|ui| {
                        range_changed |= ui.checkbox(&mut player.range.enabled, "A–B range").changed();
                        if ui.button("A — Start").on_hover_text("Set the range start to the current position").clicked() {
                            player.range.set_start(player.position, player.duration); range_changed = true;
                        }
                        if ui.button("B — End").on_hover_text("Set the range end to the current position").clicked() {
                            player.range.set_end(player.position, player.duration); range_changed = true;
                        }
                        if ui.button("Reset A–B").clicked() {
                            let repeat = player.range.repeat;
                            player.range = video::PlaybackRange::new(player.duration);
                            player.range.repeat = repeat;
                            range_changed = true;
                        }
                    });
                    let mut position = self.video_seek.unwrap_or(player.position);
                    let action = timeline::show(ui, &mut position, player.duration, &mut player.range);
                    if action.seeking {
                        if self.video_seek.is_none() { self.scrub_resume = player.playing(); player.pause(); }
                        if self.video_seek.is_none_or(|previous| (previous - position).abs() > 0.001) {
                            self.scrubber.get_or_insert_with(|| video::Scrubber::new(player.path.clone(), ctx.clone()))
                                .seek(position.min((player.duration - 0.05).max(0.0)));
                        }
                        self.video_seek = Some(position);
                        player.position = position;
                    }
                    range_changed |= action.range_committed;
                    ui.label(format!("A {}  —  B {}", timeline::timecode(player.range.start), timeline::timecode(player.range.end)));
                    if action.seek.is_some() || range_changed {
                        if let Some(position) = action.seek { player.position = position; }
                        let playing = player.playing() || self.scrub_resume;
                        self.video_seek = None; self.scrub_resume = false;
                        let (start, end) = player.range.bounds(player.duration);
                        player.position = player.position.clamp(start, (end - player.duration.min(1.0 / 24.0)).max(start));
                        player.pause();
                        if playing {
                            self.scrubber = None;
                            if let Err(error) = player.play(&ctx) { self.status = format!("Video: {error}"); }
                        } else {
                            self.scrubber.get_or_insert_with(|| video::Scrubber::new(player.path.clone(), ctx.clone()))
                                .seek(player.position.min((player.duration - 0.05).max(0.0)));
                        }
                    }
                }
    }
    fn show_image(&mut self, ui: &mut egui::Ui) {
        if let Some(model) = self.model.clone() {
            if self.fit { self.model_view = model3d::View::default(); self.fit = false; }
            if model3d::show(ui, model, &mut self.model_view, self.model_renderer.clone(), &self.settings) { self.set_fullscreen(ui.ctx(), !self.fullscreen); }
            return;
        }
        if let Some(error) = &self.model_error { ui.label(format!("3D preview unavailable: {error}")); return; }
        if self.model_request.is_some() {
            ui.centered_and_justified(|ui| { ui.label("Loading 3D model…"); });
            return;
        }
        if let Some(text) = &self.markdown {
            let rect = ui.available_rect_before_wrap();
            egui::ScrollArea::both().id_salt(("markdown", self.current, self.fullscreen)).auto_shrink([false, false]).show(ui, |ui| {
                egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.markdown_cache, text);
            });
            if ui.ctx().input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)
                && i.pointer.interact_pos().is_some_and(|pos| rect.contains(pos))) {
                self.set_fullscreen(ui.ctx(), !self.fullscreen);
            }
            return;
        }
        let avail = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(avail, Sense::click_and_drag());
        ui.painter().rect_filled(avail, 0.0, Color32::from_gray(24));

        let Some(tex) = self.texture.clone() else {
            ui.painter().text(
                avail.center(),
                Align2::CENTER_CENTER,
                "Select a file in the thumbnail grid",
                FontId::proportional(16.0),
                Color32::GRAY,
            );
            return;
        };

        let w = self.tex_size[0] as f32;
        let h = self.tex_size[1] as f32;
        if self.fit {
            self.zoom = (avail.width() / w).min(avail.height() / h);
            self.pan = Vec2::ZERO;
        }

        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 && resp.hovered() {
            let old = self.zoom;
            let new = (old * (scroll * 0.0025).exp()).clamp(0.02, 64.0);
            if let Some(mouse) = resp.hover_pos() {
                let center = avail.center() + self.pan;
                let d = mouse - center;
                self.pan -= d * (new / old - 1.0);
            }
            self.zoom = new;
            self.fit = false;
        }
        if resp.dragged() {
            self.pan += resp.drag_delta();
            self.fit = false;
        }
        if resp.double_clicked() {
            self.set_fullscreen(ui.ctx(), !self.fullscreen);
        }

        let size = Vec2::new(w, h) * self.zoom;
        let rect = Rect::from_center_size(avail.center() + self.pan, size);
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().text(
            avail.left_top() + Vec2::new(8.0, 6.0),
            Align2::LEFT_TOP,
            format!("{}×{}   {:.0}%", self.tex_size[0], self.tex_size[1], self.zoom * 100.0),
            FontId::monospace(12.0),
            Color32::from_white_alpha(180),
        );
    }
    fn poll_media(&mut self, ctx: &egui::Context) {
        self.poll_pdf_page(ctx);
        if let Some((id, result)) = self.model_loader.as_ref().and_then(model3d::Loader::poll) {
            if self.model_request == Some(id) {
                self.model_request = None;
        self.model_error = None;
                match result {
                    Ok(model) => { self.status = format!("3D model · {} triangles", model.triangles); self.model = Some(model); }
                    Err(error) => { self.status = format!("3D: {error}"); self.model_error = Some(error.to_string()); }
                }
            }
        }
        if self.video.as_ref().is_some_and(|player| player.playing()) { self.scrubber = None; }
        if let Some(result) = self.scrubber.as_ref().and_then(video::Scrubber::poll) {
            match result {
                Ok(color) => { self.tex_size = color.size; self.texture = Some(ctx.load_texture("video-scrub", color, TextureOptions::LINEAR)); }
                Err(error) => self.status = format!("Video: {error}"),
            }
        }
        if let Some(player) = &mut self.video {
            if let Some(color) = match player.poll(ctx) { Ok(frame) => frame, Err(error) => { self.status = format!("Video: {error}"); None } } {
                self.tex_size = color.size;
                if let Some(texture) = &mut self.texture { texture.set(color, TextureOptions::LINEAR); }
                else { self.texture = Some(ctx.load_texture("video-frame", color, TextureOptions::LINEAR)); }
            }
            if player.playing() { ctx.request_repaint_after(std::time::Duration::from_millis(16)); }
        }
        self.receive_thumbnails(ctx);

    }
    fn navigate_history(&mut self, delta: isize, ctx: &egui::Context) {
        let index=(self.history_index as isize+delta).clamp(0,self.history.len().saturating_sub(1) as isize) as usize;
        if index!=self.history_index && self.tabs[self.active_tab].locked {
            let node=self.history[index].clone();self.select(node,ctx);return;
        }
        if index!=self.history_index { self.history_index=index; self.select(self.history[index].clone(),ctx); }
    }
    fn navigate_up(&mut self, ctx: &egui::Context) {
        if let Some(current)=&self.selected {let nodes=current.breadcrumbs();if nodes.len()>1 {self.select(nodes[nodes.len()-2].clone(),ctx);}}
    }
    fn switch_pane(&mut self, index: usize) {
        if !self.dual_panes || index==self.active_pane {return;}
        // Both players remain visible in split preview; focus must not stop either one.
        if !self.split_preview {
            if let Some(video)=&mut self.video {video.pause();}
            self.scrubber=None;self.video_seek=None;self.scrub_resume=false;
        }
        if let Some(mut other)=self.inactive.take() {
            self.swap_browser(&mut other);
            std::mem::swap(&mut self.tabs, &mut other.tabs);
            std::mem::swap(&mut self.active_tab, &mut other.active_tab);
            self.retired_renderers.append(&mut other.retired_renderers);
            self.inactive = Some(other);
            self.active_pane=index;
        }
    }
    fn swap_browser(&mut self, other: &mut App) {
        macro_rules! swap {($($field:ident),*)=>{$(std::mem::swap(&mut self.$field,&mut other.$field);)*};}
        swap!(history,history_index,model,model_loader,model_request,model_error,model_view,model_renderer,
            pdf,pdf_scrubber,pdf_request,markdown,markdown_cache,cache_thumbnails,video_seek,scrubber,scrub_resume,
            thumb_queue,video,thumb_size,archive_errors,gallery,gallery_nodes,thumbs,thumb_errors,thumb_rx,
            thumb_generation,expanded,selected,archive_cache,images,current,texture,tex_size,zoom,fit,pan,status,pending_open);
    }
    fn switch_tab(&mut self, index: usize) {
        if index == self.active_tab || index >= self.tabs.len() { return; }
        if let Some(video) = &mut self.video { video.pause(); }
        self.scrubber = None; self.video_seek = None; self.scrub_resume = false;
        let mut state = self.tabs[index].state.take().expect("Inactive tab has browser state");
        self.swap_browser(&mut state);
        self.tabs[self.active_tab].state = Some(state);
        self.active_tab = index;
        self.cache_thumbnails = self.settings.cache;
        self.thumb_size = self.settings.thumb_size;
    }
    fn new_tab(&mut self, duplicate: bool, ctx: &egui::Context) {
        let mut state = App::new(None);
        state.settings = self.settings.clone();
        state.cache_thumbnails = self.cache_thumbnails; state.thumb_size = self.thumb_size;
        if duplicate {
            if let Some(node) = self.selected.clone() { state.select(node, ctx); }
            state.history = self.history.clone(); state.history_index = self.history_index;
            state.expanded = self.expanded.clone();
            if let Some(index) = self.current { state.goto(index, ctx); }
            state.zoom = self.zoom; state.fit = self.fit; state.pan = self.pan;
            state.model_view = self.model_view.clone();
        }
        let title = state.selected.as_ref().map(Node::label).unwrap_or_else(|| "This PC".into());
        self.tabs.push(BrowserTab { locked: false, title, state: Some(Box::new(state)) });
        self.switch_tab(self.tabs.len() - 1);
    }
    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() || self.tabs[index].locked { return; }
        if index == self.active_tab { self.switch_tab(if index > 0 { index - 1 } else { 1 }); }
        if let Some(mut state) = self.tabs.remove(index).state {
            state.thumb_generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.retired_renderers.push(state.model_renderer.clone());
            self.retired_renderers.append(&mut state.retired_renderers);
        }
        if self.active_tab > index { self.active_tab -= 1; }
    }
    fn show_tabs(&mut self, ui: &mut egui::Ui) {
        let mut switch = None; let mut close = None; let mut lock = None; let mut duplicate = None; let mut add = false;
        egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| { ui.horizontal(|ui| {
            for (index, tab) in self.tabs.iter().enumerate() {
                ui.push_id(index, |ui| {
                    let label = format!("{}{}", if tab.locked { "[Locked] " } else { "" }, tab.title);
                    let response = ui.selectable_label(index == self.active_tab, label).on_hover_text("Right-click for tab actions");
                    if response.clicked() { switch = Some(index); }
                    response.context_menu(|ui| {
                        if ui.button("New tab").clicked() { add = true; ui.close_menu(); }
                        if ui.button("Duplicate tab").clicked() { duplicate = Some(index); ui.close_menu(); }
                        if ui.button(if tab.locked { "Unlock tab" } else { "Lock tab" }).clicked() { lock = Some(index); ui.close_menu(); }
                        if ui.add_enabled(!tab.locked && self.tabs.len() > 1, egui::Button::new("Close tab")).clicked() { close = Some(index); ui.close_menu(); }
                    });
                    if ui.add_enabled(!tab.locked && self.tabs.len() > 1, egui::Button::new("×").small()).on_hover_text("Close tab").clicked() { close = Some(index); }
                });
            }
            if ui.small_button("+").on_hover_text("New tab").clicked() { add = true; }
        }); });
        if let Some(index) = switch { self.switch_tab(index); }
        if let Some(index) = lock { self.tabs[index].locked = !self.tabs[index].locked; }
        if let Some(index) = close { self.close_tab(index); }
        if let Some(index) = duplicate { self.switch_tab(index); self.new_tab(true, ui.ctx()); }
        if add { self.new_tab(false, ui.ctx()); }
    }
    fn set_dual(&mut self, enabled: bool, ctx: &egui::Context) {
        if !enabled && self.dual_panes {self.switch_pane(0);}
        if enabled && self.inactive.is_none() {
            let mut other=App::new(None);other.settings=self.settings.clone();other.cache_thumbnails=self.cache_thumbnails;other.thumb_size=self.thumb_size;
            let initial=self.pending_open.as_ref().and_then(|path|if path.is_dir(){Some(Node::Dir(path.clone()))}else{path.parent().map(|parent|Node::Dir(parent.to_path_buf()))}).or_else(||self.selected.clone());
            if let Some(node)=initial{other.select(node,ctx);}
            self.inactive=Some(Box::new(other));
        }
        self.dual_panes=enabled;self.settings.dual=enabled;
        if !enabled {self.split_preview=false;if let Some(other)=self.inactive.as_mut(){if let Some(video)=&mut other.video{video.pause();}}}
    }
    fn apply_preferences(&mut self,ctx:&egui::Context) {
        self.set_dual(self.settings.dual,ctx);
        self.thumb_size=self.settings.thumb_size;
        let reload=self.cache_thumbnails!=self.settings.cache;self.cache_thumbnails=self.settings.cache;
        if reload{self.start_thumbnails(ctx);}
        video::GPU.store(self.settings.gpu,AtomicOrdering::Relaxed);
        if let Some(other)=self.inactive.as_mut(){other.settings=self.settings.clone();other.thumb_size=self.settings.thumb_size;
            if other.cache_thumbnails!=self.settings.cache {other.cache_thumbnails=self.settings.cache;other.start_thumbnails(ctx);}
        }
    }
    fn save_preferences(&mut self) {if let Err(error)=self.settings.save(){self.status=format!("Settings: {error}");}}
    fn open_settings(&mut self) {
        self.settings.thumb_size=self.thumb_size;self.settings.cache=self.cache_thumbnails;
        self.settings.gpu=video::GPU.load(AtomicOrdering::Relaxed);self.settings.dual=self.dual_panes;
        self.settings_open=true;
    }
    fn show_navigation(&mut self,ui:&mut egui::Ui) {
        let ctx=ui.ctx().clone();
        if ui.add_enabled(self.history_index>0,egui::Button::new("< Back")).clicked(){self.navigate_history(-1,&ctx);}
        if ui.add_enabled(self.history_index+1<self.history.len(),egui::Button::new("Forward >")).clicked(){self.navigate_history(1,&ctx);}
        if ui.add_enabled(!matches!(self.selected,Some(Node::Computer)),egui::Button::new("^ Up")).clicked(){self.navigate_up(&ctx);}
        if ui.button("Refresh").clicked(){self.archive_cache.clear();if let Some(node)=self.selected.clone(){self.select(node,&ctx);}}
    }
    fn show_browser_pane(&mut self,ui:&mut egui::Ui,index:usize,active:bool) {
        self.show_tabs(ui);
        ui.horizontal_wrapped(|ui|{ui.strong(format!("Pane {}{}",index+1,if active{" · Active"}else{""}));self.show_navigation(ui);});
        let current=self.selected.clone().unwrap_or(Node::Computer);let mut target=None;
        egui::ScrollArea::horizontal().id_salt("pane_path").show(ui,|ui|{ui.horizontal(|ui|{
            for (i,node) in current.breadcrumbs().into_iter().enumerate(){if i>0{ui.label("›");}
                if ui.selectable_label(node==current,node.label()).clicked(){target=Some(node);}
            }
        });});
        if let Some(node)=target{self.select(node,ui.ctx());}
        ui.separator();ui.push_id(self.active_tab, |ui| self.show_gallery(ui));
    }
    fn show_panes(&mut self,ui:&mut egui::Ui) {
        if !self.dual_panes {ui.push_id("single_browser",|ui|self.show_browser_pane(ui,0,true));return;}
        let rect=ui.available_rect_before_wrap();let split=rect.left()+rect.width()*self.pane_fraction;
        let left=Rect::from_min_max(rect.min,Pos2::new(split-3.0,rect.bottom()));
        let right=Rect::from_min_max(Pos2::new(split+3.0,rect.top()),rect.max);
        let divider=Rect::from_min_max(Pos2::new(split-3.0,rect.top()),Pos2::new(split+3.0,rect.bottom()));
        let response=ui.interact(divider,ui.id().with("browser_split"),Sense::drag());
        if response.dragged(){self.pane_fraction=(self.pane_fraction+ui.ctx().input(|i|i.pointer.delta().x)/rect.width().max(1.0)).clamp(0.2,0.8);}
        response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        ui.painter().rect_filled(divider,0.0,Color32::from_gray(65));
        if let Some(pos)=ui.ctx().input(|i|if i.pointer.any_pressed(){i.pointer.interact_pos()}else{None}) {
            if left.contains(pos){self.switch_pane(0);}else if right.contains(pos){self.switch_pane(1);}
        }
        for (index,area) in [left,right].into_iter().enumerate(){
            let active=self.active_pane==index;
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(area),|ui|{ui.push_id(("browser_pane",index),|ui|{
                egui::Frame::none().inner_margin(4.0).stroke(egui::Stroke::new(1.0_f32,if active{Color32::from_rgb(65,140,200)}else{Color32::from_gray(45)})).show(ui,|ui|{
                    if active {self.show_browser_pane(ui,index,true);}else if let Some(other)=self.inactive.as_mut(){other.show_browser_pane(ui,index,false);}
                });
            });});
        }
    }
    fn show_preview_content(&mut self, ui: &mut egui::Ui, pane: usize) {
        ui.horizontal(|ui| {
            ui.strong(format!("Pane {}", pane + 1));
            if ui.small_button("Fit").clicked() { self.fit = true; }
            if self.model.is_none() && self.model_request.is_none() && ui.small_button("1:1").clicked() {
                self.fit = false; self.zoom = 1.0; self.pan = Vec2::ZERO;
            }
        });
        if let Some(image) = self.current.and_then(|index| self.images.get(index)) { ui.label(&image.label); }
        self.show_video_controls(ui);
        self.show_document_controls(ui);
        ui.separator();
        self.show_image(ui);
    }
    fn show_preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Preview");
            let mut split=self.split_preview;
            if ui.toggle_value(&mut split, "Split preview").on_hover_text("Pane 1 above, Pane 2 below").changed() {
                self.set_split_preview(split, ui.ctx());
                self.save_preferences();
            }
        });
        ui.separator();
        if !self.split_preview {
            ui.push_id(("preview",self.active_pane,self.active_tab), |ui| self.show_preview_content(ui,self.active_pane));
            return;
        }
        let rect = ui.available_rect_before_wrap();
        let y = rect.top() + rect.height() * self.preview_fraction;
        let divider = Rect::from_min_max(Pos2::new(rect.left(),y-3.0), Pos2::new(rect.right(),y+3.0));
        let drag = ui.interact(divider, ui.id().with("preview_split"), Sense::drag());
        if drag.dragged() { self.preview_fraction=(self.preview_fraction + ui.ctx().input(|i| i.pointer.delta().y)/rect.height().max(1.0)).clamp(0.2,0.8); }
        drag.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        ui.painter().rect_filled(divider,0.0,Color32::from_gray(65));
        let areas = [Rect::from_min_max(rect.min,Pos2::new(rect.right(),y-3.0)), Rect::from_min_max(Pos2::new(rect.left(),y+3.0),rect.max)];
        if let Some(pos)=ui.ctx().input(|i| if i.pointer.any_pressed(){i.pointer.interact_pos()}else{None}) {
            for (index, area) in areas.iter().enumerate() { if area.contains(pos) { self.switch_pane(index); } }
        }
        for (index,area) in areas.into_iter().enumerate() {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(area), |ui| {
                ui.set_clip_rect(area.intersect(ui.clip_rect()));
                if index==self.active_pane {
                    ui.push_id(("preview",index,self.active_tab), |ui| self.show_preview_content(ui,index));
                } else if let Some(other)=self.inactive.as_mut() {
                    other.settings=self.settings.clone();
                    ui.push_id(("preview",index,other.active_tab), |ui| other.show_preview_content(ui,index));
                }
            });
        }
    }
    fn set_split_preview(&mut self, enabled: bool, ctx: &egui::Context) {
        self.split_preview=enabled;
        if enabled {self.set_dual(true,ctx);}
        else if let Some(other)=self.inactive.as_mut() {
            if let Some(player)=&mut other.video {player.pause();}
            other.scrubber=None;other.video_seek=None;other.scrub_resume=false;
        }
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        for tab in &mut self.tabs { if let Some(state)=&mut tab.state {eframe::App::on_exit(state.as_mut(),gl);} }
        if let Some(gl)=gl {for renderer in self.retired_renderers.drain(..){if let Some(gpu)=renderer.lock().unwrap().take(){unsafe{gpu.destroy(gl);}}}}
        if let Some(other)=self.inactive.as_mut(){eframe::App::on_exit(other.as_mut(),gl);}
        if let Some(gl) = gl { if let Some(gpu) = self.model_renderer.lock().unwrap().take() { unsafe { gpu.destroy(gl); } } }
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(other)=self.inactive.as_mut(){self.retired_renderers.append(&mut other.retired_renderers);}
        if let Some(gl)=_frame.gl() {for renderer in self.retired_renderers.drain(..){if let Some(gpu)=renderer.lock().unwrap().take(){unsafe{gpu.destroy(gl);}}}}
        if let Some(path)=std::env::var_os("ARCHVIEW_UI_CAPTURE") {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            static REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            let screenshot=ctx.input(|i|i.events.iter().find_map(|event|if let egui::Event::Screenshot{image,..}=event{Some(image.clone())}else{None}));
            if let Some(image)=screenshot {
                let pixels:Vec<u8>=image.pixels.iter().flat_map(|color|color.to_array()).collect();
                if let Some(image)=image::RgbaImage::from_raw(image.size[0] as u32,image.size[1] as u32,pixels){let _=image.save(path);}
            } else if START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32()>2.0 && !REQUESTED.swap(true,AtomicOrdering::Relaxed) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            } else {ctx.request_repaint_after(std::time::Duration::from_millis(100));}
        }
        if !self.settings_initialized {self.settings_initialized=true;self.apply_preferences(ctx);}
        self.poll_media(ctx);
        if let Some(other)=self.inactive.as_mut(){other.poll_media(ctx);}
        let settings_were_open=self.settings_open;
        if self.settings_open {
            if self.settings.window(ctx,&mut self.settings_open,&mut self.key_capture,&mut self.settings_message){self.apply_preferences(ctx);self.save_preferences();}
        }
        if !settings_were_open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.fullscreen { self.set_fullscreen(ctx, false); }
            self.gallery = true;
        }
        // отложенное открытие (стартовый путь / drag&drop / диалог)
        if let Some(p) = self.pending_open.take() {
            self.open_path(&p, ctx);
            if std::env::var_os("ARCHVIEW_UI_CAPTURE").is_some() && std::env::var("ARCHVIEW_UI_CHECK").is_ok_and(|mode|mode.contains("split")) {
                self.goto(0,ctx);self.new_tab(true,ctx);self.tabs[0].locked=true;
                if let Some(other)=self.inactive.as_mut(){other.goto(other.images.len().saturating_sub(1),ctx);}
            }
        }
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.first() {
            self.open_path(p, ctx);
        }

        if !settings_were_open && !ctx.wants_keyboard_input() {
            if self.settings.pressed(Action::Settings,ctx){self.open_settings();}
            else {
                if self.settings.pressed(Action::Next,ctx) || self.settings.pressed(Action::NextAlternate,ctx){self.step(1,ctx);}
                if self.settings.pressed(Action::Previous,ctx) || self.settings.pressed(Action::PreviousAlternate,ctx){self.step(-1,ctx);}
                if self.settings.pressed(Action::First,ctx){if self.pdf.is_some(){self.load_pdf_page(0,ctx);}else{self.goto(0,ctx);}}
                if self.settings.pressed(Action::Last,ctx){if let Some((_,_,count))=self.pdf.clone(){self.load_pdf_page(count-1,ctx);}else if !self.images.is_empty(){self.goto(self.images.len()-1,ctx);}}
                if self.settings.pressed(Action::Fit,ctx){self.fit=true;}
                if self.settings.pressed(Action::ActualSize,ctx){self.fit=false;self.zoom=1.0;self.pan=Vec2::ZERO;}
                if self.settings.pressed(Action::PlayPause,ctx){if let Some(player)=&mut self.video{if player.playing(){player.pause();}else if let Err(error)=player.play(ctx){self.status=format!("Video: {error}");}}}
                if self.settings.pressed(Action::Fullscreen,ctx){self.set_fullscreen(ctx,!self.fullscreen);}
                if self.settings.pressed(Action::Back,ctx){self.navigate_history(-1,ctx);}
                if self.settings.pressed(Action::Forward,ctx){self.navigate_history(1,ctx);}
                if self.settings.pressed(Action::Up,ctx){self.navigate_up(ctx);}
                if self.settings.pressed(Action::Dual,ctx){self.set_dual(!self.dual_panes,ctx);self.save_preferences();}
                if self.settings.pressed(Action::SwitchPane,ctx){self.switch_pane(1-self.active_pane);}
                if self.settings.pressed(Action::NewTab,ctx){self.new_tab(false,ctx);}
                if self.settings.pressed(Action::CloseTab,ctx){self.close_tab(self.active_tab);}
                if self.settings.pressed(Action::DuplicateTab,ctx){self.new_tab(true,ctx);}
                if self.settings.pressed(Action::LockTab,ctx){self.tabs[self.active_tab].locked=!self.tabs[self.active_tab].locked;}
            }
        }
        if self.fullscreen {
            egui::TopBottomPanel::bottom("fullscreen_controls").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Exit fullscreen (Esc)").clicked() { self.set_fullscreen(ctx, false); }
                    if let Some(i) = self.current {
                        if let Some(media) = self.images.get(i) { ui.label(&media.label); }
                    }
                });
                self.show_video_controls(ui);
                self.show_document_controls(ui);
            });
            egui::CentralPanel::default().frame(egui::Frame::none())
                .show(ctx, |ui| self.show_image(ui));
            return;
        }

        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                self.show_navigation(ui);
                if ui.selectable_label(self.dual_panes,"Dual panes").clicked(){self.set_dual(!self.dual_panes,ctx);self.save_preferences();}
                if ui.button("Settings…").clicked(){self.open_settings();}
                ui.separator();
                if ui.button("This PC").clicked() {
                    self.select(Node::Computer, ctx);
                }
                if ui.button("Open folder…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.pending_open = Some(p);
                    }
                }
                if ui.button("Open archive…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("archives", ARCHIVE_EXT)
                        .pick_file()
                    {
                        self.pending_open = Some(p);
                    }
                }
                if ui.button("Open image…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Images", IMAGE_EXT)
                        .pick_file()
                    {
                        self.pending_open = Some(p);
                    }
                }
                if ui.button("Open video…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Video", VIDEO_EXT).pick_file() {
                        self.pending_open = Some(path);
                    }
                }
                if ui.button("Open 3D…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("3D models", model3d::EXTENSIONS).pick_file() { self.pending_open = Some(path); }
                }
                if ui.button("Open document…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("PDF / Markdown", DOCUMENT_EXT).pick_file() {
                        self.pending_open = Some(path);
                    }
                }
                ui.separator();
                if ui.button("Thumbnails").clicked() { self.gallery = true; }
            });
            ui.separator();
            let current = self.selected.clone().unwrap_or(Node::Computer);
            let breadcrumbs = current.breadcrumbs();
            let mut destination = None;
            egui::ScrollArea::horizontal().id_salt("path_breadcrumbs")
                .auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, node) in breadcrumbs.iter().enumerate() {
                            if index > 0 { ui.label(RichText::new("›").weak()); }
                            let response = ui.selectable_label(node == &current, node.label());
                            if response.clicked() { destination = Some(node.clone()); }
                            response.on_hover_text(match node {
                                Node::Dir(path) | Node::Archive(path) => path.display().to_string(),
                                Node::ArchiveDir { archive, prefix } => format!("{} / {}", archive.display(), prefix),
                                Node::Computer => "This PC".into(),
                            });
                        }
                    });
                });
            if let Some(node) = destination { self.select(node, ctx); }
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(RichText::new(&self.status).small());
        });

        let mut tree_click: Option<Node> = None;

        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                let bookmark_click = egui::TopBottomPanel::bottom("bookmarks")
                    .resizable(true).default_height(200.0).min_height(80.0)
                    .show_inside(ui, |ui| self.show_bookmarks(ui)).inner;
                if bookmark_click.is_some() { tree_click = bookmark_click; }
                ui.horizontal(|ui| {
                    ui.strong("Folders and archives");
                    if ui.small_button("Collapse all").clicked() {
                        self.expanded.clear();
                    }
                });
                ui.separator();
                egui::ScrollArea::both().id_salt("tree_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    let root = Node::Computer;
                    if let Some(node) = self.show_tree(ui, &root, 0) { tree_click = Some(node); }
                });
            });

        if let Some(n) = tree_click {
            self.select(n, ctx);
        }

        egui::SidePanel::right("preview")
            .resizable(true).default_width(400.0).min_width(180.0)
            .show(ctx, |ui| {
                self.show_preview_panel(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.show_panes(ui);
            });
        let gpu=video::GPU.load(AtomicOrdering::Relaxed);
        if self.settings.thumb_size!=self.thumb_size || self.settings.cache!=self.cache_thumbnails || self.settings.gpu!=gpu {
            self.settings.thumb_size=self.thumb_size;self.settings.cache=self.cache_thumbnails;self.settings.gpu=gpu;self.save_preferences();
        }
    }
}

fn main() -> eframe::Result<()> {
    let start = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        depth_buffer: 24,
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("view-calipers")
            .with_inner_size([1400.0, 900.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "view-calipers",
        options,
        Box::new(move |_cc| {let mut app=App::new(start);app.settings=settings::Settings::load();if std::env::var_os("ARCHVIEW_UI_CAPTURE").is_some(){if let Ok(mode)=std::env::var("ARCHVIEW_UI_CHECK"){app.settings.dual=mode.contains("dual")||mode.contains("split");app.split_preview=mode.contains("split");app.settings_open=mode.contains("settings");}}Ok(Box::new(app))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn split_preview_plays_both_videos_and_pauses_each_independently() {
        let root=std::env::temp_dir().join(format!("view-calipers-two-videos-{}",std::process::id()));
        let left=root.join("left");let right=root.join("right");
        std::fs::create_dir_all(&left).unwrap();std::fs::create_dir_all(&right).unwrap();
        for (folder,size) in [(&left,"160x90"),(&right,"90x160")] {
            let output=video::command("ffmpeg").args(["-y","-v","error","-f","lavfi","-i",&format!("testsrc=size={size}:rate=24"),"-t","3","-c:v","mpeg4","-pix_fmt","yuv420p"])
                .arg(folder.join("motion.mp4")).output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        }
        let ctx=egui::Context::default();let mut app=App::new(None);
        app.select(Node::Dir(left.clone()),&ctx);app.goto(0,&ctx);
        app.set_split_preview(true,&ctx);
        app.video.as_mut().unwrap().muted=true;
        app.video.as_mut().unwrap().play(&ctx).unwrap();
        app.switch_pane(1);
        assert!(app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        app.select(Node::Dir(right),&ctx);app.goto(0,&ctx);
        app.video.as_mut().unwrap().muted=true;
        app.video.as_mut().unwrap().play(&ctx).unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(8);
        loop {
            app.poll_media(&ctx);app.inactive.as_mut().unwrap().poll_media(&ctx);
            if app.video.as_ref().unwrap().position>0.1 && app.inactive.as_ref().unwrap().video.as_ref().unwrap().position>0.1 {break;}
            assert!(std::time::Instant::now()<deadline,"Both players must advance");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.video.as_ref().unwrap().playing());
        assert!(app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        assert!(app.tex_size[0]<app.tex_size[1]);
        assert!(app.inactive.as_ref().unwrap().tex_size[0]>app.inactive.as_ref().unwrap().tex_size[1]);
        app.video.as_mut().unwrap().pause();
        app.switch_pane(0);
        assert!(app.video.as_ref().unwrap().playing());
        assert!(!app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        app.switch_pane(1);app.video.as_mut().unwrap().play(&ctx).unwrap();app.switch_pane(0);
        assert!(app.video.as_ref().unwrap().playing());
        assert!(app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        app.set_split_preview(false,&ctx);
        assert!(app.video.as_ref().unwrap().playing());
        assert!(!app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        app.switch_pane(1);
        assert!(!app.inactive.as_ref().unwrap().video.as_ref().unwrap().playing());
        drop(app);std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tabs_preserve_selected_media_and_locked_paths_across_panes() {
        let root=std::env::temp_dir().join(format!("archview-tabs-{}",std::process::id()));
        let left=root.join("left");let right=root.join("right");
        std::fs::create_dir_all(&left).unwrap();std::fs::create_dir_all(&right).unwrap();
        image::RgbImage::new(24,12).save(left.join("left.png")).unwrap();
        image::RgbImage::new(12,24).save(right.join("right.png")).unwrap();
        let ctx=egui::Context::default();let mut app=App::new(None);
        app.select(Node::Dir(left.clone()),&ctx);app.goto(0,&ctx);
        app.new_tab(true,&ctx);
        assert_eq!(app.tabs.len(),2);assert_eq!(app.current,Some(0));assert_eq!(app.tex_size,[24,12]);
        app.select(Node::Dir(right.clone()),&ctx);app.goto(0,&ctx);
        app.switch_tab(0);assert_eq!(app.selected,Some(Node::Dir(left.clone())));assert_eq!(app.current,Some(0));assert_eq!(app.tex_size,[24,12]);
        app.tabs[0].locked=true;
        let history=app.history.clone();let history_index=app.history_index;
        app.navigate_up(&ctx);assert_eq!(app.tabs.len(),3);assert_eq!(app.selected,Some(Node::Dir(root.clone())));
        app.close_tab(0);assert_eq!(app.tabs.len(),3);
        app.switch_tab(0);assert_eq!(app.selected,Some(Node::Dir(left.clone())));assert_eq!(app.history,history);assert_eq!(app.history_index,history_index);
        app.set_dual(true,&ctx);app.split_preview=true;app.switch_pane(1);
        assert_eq!(app.tabs.len(),1);app.new_tab(false,&ctx);assert_eq!(app.tabs.len(),2);
        app.switch_pane(0);assert_eq!(app.tabs.len(),3);assert!(app.tabs[0].locked);assert_eq!(app.current,Some(0));
        app.close_tab(2);app.close_tab(1);assert_eq!(app.tabs.len(),1);assert_eq!(app.current,Some(0));
        app.close_tab(0);assert_eq!(app.tabs.len(),1);
        let _=ctx.run(egui::RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,Vec2::new(1400.0,900.0))),..Default::default()},|ctx| {
            egui::CentralPanel::default().show(ctx,|ui| app.show_preview_panel(ui));
        });
        assert_eq!(app.active_pane,0);assert_eq!(app.current,Some(0));
        app.set_dual(false,&ctx);assert!(!app.split_preview);assert_eq!(app.current,Some(0));
        drop(app);std::thread::sleep(std::time::Duration::from_millis(100));std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exif_orientation_applies_to_preview_folder_archive_and_cached_thumbnails() {
        let root = std::env::temp_dir().join(format!("archview-exif-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let photo = root.join("portrait.jpg");
        let archive = root.join("portrait.cbz");
        let bytes = image_decode::jpeg_with_orientation(6);
        std::fs::write(&photo, &bytes).unwrap();
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer.start_file("pages/portrait.jpg", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let sources = [
            folder_cover(&Node::Dir(root.clone())).unwrap(),
            folder_cover(&Node::Archive(archive)).unwrap(),
        ];
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        for source in sources {
            let cache = thumbnail_cache::key(&source, &thumbnail_cache::directory()).unwrap();
            let thumbnail = decode_thumbnail(&source, true).unwrap();
            assert_eq!(thumbnail.size, [16, 24]);
            assert!(cache.exists());
            assert_eq!(decode_thumbnail(&source, true).unwrap().pixels, thumbnail.pixels);
            app.images = vec![ImageRef { label: "portrait.jpg".into(), src: source }];
            app.goto(0, &ctx);
            assert_eq!(app.tex_size, [16, 24]);
            app.set_fullscreen(&ctx, true);
            assert_eq!(app.tex_size, [16, 24]);
            app.set_fullscreen(&ctx, false);
            std::fs::remove_file(cache).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dual_panes_keep_independent_paths_history_and_selected_media() {
        let root=std::env::temp_dir().join(format!("archview-panes-{}",std::process::id()));
        let left=root.join("left");let right=root.join("right");
        std::fs::create_dir_all(&left).unwrap();std::fs::create_dir_all(&right).unwrap();
        image::RgbaImage::from_pixel(16,8,image::Rgba([255,0,0,255])).save(left.join("left.png")).unwrap();
        image::RgbaImage::from_pixel(8,16,image::Rgba([0,0,255,255])).save(right.join("right.png")).unwrap();
        let ctx=egui::Context::default();let mut app=App::new(None);
        app.select(Node::Dir(left.clone()),&ctx);app.goto(0,&ctx);app.set_dual(true,&ctx);
        app.switch_pane(1);app.select(Node::Dir(right.clone()),&ctx);app.goto(0,&ctx);
        assert_eq!(app.tex_size,[8,16]);
        app.switch_pane(0);assert_eq!(app.selected,Some(Node::Dir(left.clone())));assert_eq!(app.current,Some(0));assert_eq!(app.tex_size,[16,8]);
        app.navigate_history(-1,&ctx);assert_eq!(app.selected,Some(Node::Computer));
        app.navigate_history(1,&ctx);assert_eq!(app.selected,Some(Node::Dir(left.clone())));
        app.switch_pane(1);assert_eq!(app.selected,Some(Node::Dir(right.clone())));assert_eq!(app.current,Some(0));
        app.set_dual(false,&ctx);assert_eq!(app.active_pane,0);assert_eq!(app.selected,Some(Node::Dir(left)));
        app.set_dual(true,&ctx);app.switch_pane(1);assert_eq!(app.selected,Some(Node::Dir(right)));
        app.navigate_history(-1,&ctx);app.select(Node::Dir(root.clone()),&ctx);
        assert_eq!(app.history_index+1,app.history.len());
        while app.thumb_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(5)).is_ok(){}
        if let Some(rx)=&app.inactive.as_ref().unwrap().thumb_rx{while rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok(){}}
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pdf_and_markdown_are_listed_thumbnailed_and_viewed() {
        let root = std::env::temp_dir().join(format!("archview-documents-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let pdf = root.join("pages.pdf");
        let markdown = root.join("readme.md");
        let objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 7 0 R >> >> /Contents 4 0 R >>".into(),
            { let text = "BT /F1 20 Tf 20 100 Td (First page) Tj ET"; format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()) },
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>".into(),
            { let text = "BT /F1 20 Tf 20 100 Td (Second page) Tj ET"; format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()) },
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        let mut data = "%PDF-1.4\n".to_string();
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() { offsets.push(data.len()); data.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1)); }
        let xref = data.len();
        data.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
        for offset in offsets.iter().skip(1) { data.push_str(&format!("{offset:010} 00000 n \n")); }
        data.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF", offsets.len()));
        std::fs::write(&pdf, data).unwrap();
        std::fs::write(&markdown, "# Пример\n\n**Жирный текст**\n\n- Пункт\n\n```rust\nlet a = 1;\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.cache_thumbnails = false;
        app.select(Node::Dir(root.clone()), &ctx);
        assert_eq!(app.images.len(), 2);
        for _ in 0..2 { assert!(app.thumb_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(15)).unwrap().1.is_ok()); }
        app.goto(0, &ctx);
        assert_eq!(app.pdf.as_ref().unwrap().2, 2);
        let first = app.tex_size;
        app.load_pdf_page(1, &ctx);
        assert_eq!(app.pdf.as_ref().unwrap().1, 1);
        assert_eq!(app.tex_size, first);
        app.request_pdf_page(0, 900, &ctx);
        app.request_pdf_page(1, 900, &ctx);
        app.request_pdf_page(1, 1600, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut final_frame = None;
        while std::time::Instant::now() < deadline {
            if let Some((page, width, result)) = app.pdf_scrubber.as_ref().unwrap().poll() {
                let frame = result.unwrap();
                if (page, width) == (1, 1600) { final_frame = Some(frame); break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(final_frame.expect("Final PDF scrub frame").size, first);
        assert_eq!(app.pdf.as_ref().unwrap().1, 1);
        app.goto(1, &ctx);
        assert!(app.pdf.is_none());
        assert!(app.pdf_scrubber.is_none());
        assert!(app.markdown.as_ref().unwrap().starts_with("# Пример"));
        let _ = ctx.run(egui::RawInput::default(), |ctx| { egui::CentralPanel::default().show(ctx, |ui| app.show_image(ui)); });
        app.set_fullscreen(&ctx, true);
        assert!(app.markdown.is_some());
        app.set_fullscreen(&ctx, false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_shows_computer_without_folder_dialog_or_pending_path() {
        let app = App::new(None);
        assert_eq!(app.selected, Some(Node::Computer));
        assert!(app.expanded.contains(&Node::Computer));
        assert!(app.gallery);
        assert!(app.pending_open.is_none());
        assert_eq!(app.gallery_nodes, computer_drives());
    }

    #[test]
    fn bookmarks_persist_unicode_paths_and_removal() {
        let path = std::env::temp_dir().join(format!("archview-bookmarks-{}.txt", std::process::id()));
        let folders = vec![PathBuf::from("C:\\Фото\\Эскизы"), PathBuf::from("D:\\Reference images")];
        write_bookmarks(&path, &folders).unwrap();
        assert_eq!(read_bookmarks(&path), folders);
        write_bookmarks(&path, &folders[1..]).unwrap();
        assert_eq!(read_bookmarks(&path), folders[1..]);
        write_bookmarks(&path, &[]).unwrap();
        assert!(read_bookmarks(&path).is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn archive_gallery_includes_images_inside_nested_folders() {
        let path = std::env::temp_dir().join(format!("archview-nested-{}.cbz", std::process::id()));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(16, 12).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for name in ["root.png", "album/chapter/page2.PNG"] {
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(png.get_ref()).unwrap();
        }
        writer.finish().unwrap();
        let mut app = App::new(None);
        app.select(Node::Archive(path.clone()), &egui::Context::default());
        assert_eq!(app.images.len(), 2);
        assert_eq!(app.images[0].label, "album/chapter/page2.PNG");
        for _ in 0..3 {
            let (_, result) = app.thumb_rx.as_ref().unwrap()
                .recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(result.unwrap().size, [16, 12]);
        }
        assert_eq!(app.archive_images(&path, "album/").len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unreadable_archive_reports_error_and_can_be_retried() {
        let path = std::env::temp_dir().join(format!("archview-invalid-{}.zip", std::process::id()));
        std::fs::write(&path, [0_u8; 64]).unwrap();
        let mut app = App::new(None);
        app.select(Node::Archive(path.clone()), &egui::Context::default());
        assert!(app.status.starts_with("Could not read archive"));
        assert!(app.archive_errors.contains_key(&path));
        assert!(!app.archive_cache.contains_key(&path));
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer.start_file("folder/info.txt", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(b"valid archive").unwrap();
        writer.finish().unwrap();
        app.select(Node::Archive(path.clone()), &egui::Context::default());
        assert!(!app.archive_errors.contains_key(&path));
        assert_eq!(app.gallery_nodes.len(), 1);
        while app.thumb_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(5)).is_ok() {}
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn folder_thumbnail_uses_first_image_in_natural_order_and_empty_folder_has_no_cover() {
        let directory = std::env::temp_dir().join(format!("archview-folder-cover-{}", std::process::id()));
        let folder = directory.join("photos");
        let empty = directory.join("empty");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        image::RgbaImage::from_pixel(16, 12, image::Rgba([255, 0, 0, 255])).save(folder.join("page10.png")).unwrap();
        image::RgbaImage::from_pixel(16, 12, image::Rgba([0, 0, 255, 255])).save(folder.join("page2.png")).unwrap();
        assert!(matches!(folder_cover(&Node::Dir(folder.clone())), Some(ImageSrc::File(path)) if path == folder.join("page2.png")));
        assert!(folder_cover(&Node::Dir(empty)).is_none());
        let mut app = App::new(None);
        app.select(Node::Dir(directory.clone()), &egui::Context::default());
        assert!(app.images.is_empty());
        assert_eq!(app.gallery_nodes.len(), 2);
        let (index, thumbnail) = app.thumb_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(index, 1);
        assert_eq!(thumbnail.unwrap().pixels[0], Color32::BLUE);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn comic_rar_lists_and_reads_entries_without_extracting() {
        let path = std::env::temp_dir().join(format!("archview-comic-{}.cbr", std::process::id()));
        std::fs::write(&path, include_bytes!("../tests/fixtures/version.cbr")).unwrap();
        assert_eq!(index_archive(&path).unwrap(), vec!["VERSION"]);
        assert_eq!(read_archive_entry(&path, "VERSION").unwrap(), b"unrar-0.4.0");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn video_in_folder_has_thumbnail_playback_pause_and_seek() {
        let directory = std::env::temp_dir().join(format!("archview-video-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("motion.mp4");
        let output = video::command("ffmpeg").args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=160x120:rate=24",
            "-f", "lavfi", "-i", "sine=frequency=440", "-t", "1", "-c:v", "mpeg4", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let mut app = App::new(None);
        let ctx = egui::Context::default();
        app.select(Node::Dir(directory.clone()), &ctx);
        assert_eq!(app.images.len(), 1);
        let (_, thumbnail) = app.thumb_rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(thumbnail.is_ok());
        app.goto(0, &ctx);
        assert!(app.texture.is_some());
        let player = app.video.as_mut().unwrap();
        assert!((player.duration - 1.0).abs() < 0.2);
        player.muted = true;
        player.play(&ctx).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut received = 0;
        while received < 2 && std::time::Instant::now() < deadline {
            if let Some(frame) = player.poll(&ctx).unwrap() { assert_eq!(frame.size, player.frame_size); received += 1; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(received, 2);
        assert!(player.position > 0.0);
        player.pause();
        assert!(!player.playing());
        player.position = 0.5;
        assert!(video::poster(&path, player.position, 320, 240).is_ok());
        let scrubber = video::Scrubber::new(path.clone(), ctx.clone());
        for i in 0..20 { scrubber.seek(i as f64 / 40.0); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut scrub_frame = None;
        while scrub_frame.is_none() && std::time::Instant::now() < deadline {
            scrub_frame = scrubber.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(scrub_frame.expect("Asynchronous scrubbing frame").unwrap().size, player.frame_size);
        drop(scrubber);
        player.range.set_start(0.2, player.duration);
        player.range.set_end(0.5, player.duration);
        player.position = 0.0;
        player.play(&ctx).unwrap();
        assert!((player.position - 0.2).abs() < 0.001);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while player.playing() && std::time::Instant::now() < deadline {
            player.poll(&ctx).unwrap();
            assert!(player.position >= 0.2 && player.position <= 0.5);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!player.playing());
        assert_eq!(player.position, 0.5);
        player.range.repeat = true;
        player.play(&ctx).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut previous = player.position;
        let mut looped = false;
        while !looped && std::time::Instant::now() < deadline {
            player.poll(&ctx).unwrap();
            looped = player.position < previous;
            previous = player.position;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(looped);
        assert!(player.playing());
        player.pause();
        player.range.enabled = false;
        player.position = 0.8;
        player.play(&ctx).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut looped = false;
        while !looped && std::time::Instant::now() < deadline {
            player.poll(&ctx).unwrap();
            looped = player.position < 0.8;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(looped);
        player.pause();
        player.range.repeat = false;
        let audio_check = video::command("ffplay").args(["-v", "error", "-nodisp", "-vn", "-autoexit", "-volume", "0", "-t", "0.1"])
            .arg(&path).output().unwrap();
        assert!(audio_check.status.success(), "{}", String::from_utf8_lossy(&audio_check.stderr));
        app.video.as_mut().unwrap().play(&ctx).unwrap();
        let position = app.video.as_ref().unwrap().position;
        app.set_fullscreen(&ctx, true);
        assert!(app.fullscreen);
        assert!(app.video.as_ref().unwrap().playing());
        assert_eq!(app.video.as_ref().unwrap().position, position);
        app.set_fullscreen(&ctx, false);
        assert!(!app.fullscreen);
        assert!(app.video.as_ref().unwrap().playing());
        app.video = None;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn portrait_and_rotation_metadata_keep_same_geometry_during_playback_and_seek() {
        let directory = std::env::temp_dir().join(format!("archview-orientation-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let portrait = directory.join("portrait.mp4");
        let landscape = directory.join("landscape.mp4");
        let rotated = directory.join("rotated.mp4");
        for (path, pattern) in [(&portrait, "testsrc=size=80x160:rate=24"), (&landscape, "testsrc=size=160x90:rate=24")] {
            let output = video::command("ffmpeg").args(["-y", "-v", "error", "-f", "lavfi", "-i", pattern,
                "-t", "1", "-c:v", "mpeg4", "-pix_fmt", "yuv420p"]).arg(path).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let output = video::command("ffmpeg").args(["-y", "-v", "error", "-display_rotation:v:0", "90", "-i"]).arg(&landscape)
            .args(["-c", "copy"]).arg(&rotated).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let ctx = egui::Context::default();
        for path in [&portrait, &rotated] {
            let poster = image::load_from_memory(&video::poster(path, 0.0, 960, 540).unwrap()).unwrap();
            let size = [poster.width() as usize, poster.height() as usize];
            assert!(size[1] > size[0], "Expected a portrait poster: {size:?}");
            let mut player = video::Player::new(path.clone(), size).unwrap();
            player.muted = true;
            for position in [0.0, 0.5] {
                player.position = position;
                player.play(&ctx).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut frame = None;
                while frame.is_none() && std::time::Instant::now() < deadline {
                    frame = player.poll(&ctx).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                assert_eq!(frame.expect("Video frame did not arrive").size, size);
                player.pause();
                let seek = image::load_from_memory(&video::poster(path, position, 960, 540).unwrap()).unwrap();
                assert_eq!([seek.width() as usize, seek.height() as usize], size);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn zip_entries_with_backslashes_can_be_read() {
        let path = std::env::temp_dir().join(format!("archview-test-{}.zip", std::process::id()));
        let result = (|| -> Result<()> {
            let mut writer = zip::ZipWriter::new(File::create(&path)?);
            writer.start_file("folder\\page2.png", zip::write::SimpleFileOptions::default())?;
            writer.write_all(b"test image bytes")?;
            writer.finish()?;
            assert_eq!(index_zip(&path)?, vec!["folder/page2.png"]);
            assert_eq!(read_zip(&path, "folder/page2.png")?, b"test image bytes");
            Ok(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }

    #[test]
    fn natural_sort_handles_large_numbers_and_cyrillic() {
        let mut names = vec!["кадр10.png", "кадр999999999999999999999.png", "Кадр2.png"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["Кадр2.png", "кадр10.png", "кадр999999999999999999999.png"]);
    }

    #[test]
    fn selecting_folder_shows_gallery_and_loads_thumbnail_in_background() {
        let path = std::env::temp_dir().join(format!("archview-gallery-{}", std::process::id()));
        std::fs::create_dir_all(path.join("subfolder")).unwrap();
        image::RgbaImage::new(320, 240).save(path.join("page.png")).unwrap();
        let mut app = App::new(None);
        app.select(Node::Dir(path.clone()), &egui::Context::default());
        assert!(app.gallery);
        assert!(app.current.is_none());
        assert!(app.texture.is_none());
        assert_eq!(app.images.len(), 1);
        assert_eq!(app.gallery_nodes, vec![Node::Dir(path.join("subfolder"))]);
        let (index, result) = app.thumb_rx.as_ref().unwrap()
            .recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(index, 0);
        assert_eq!(result.unwrap().size, [320, 240]);
        app.goto(0, &egui::Context::default());
        assert!(app.gallery);
        assert!(app.texture.is_some());
        assert_eq!(app.current, Some(0));
        app.set_fullscreen(&egui::Context::default(), true);
        assert!(app.fullscreen);
        assert!(app.texture.is_some());
        app.set_fullscreen(&egui::Context::default(), false);
        assert!(!app.fullscreen);
        assert_eq!(app.current, Some(0));
        let subfolder = Node::Dir(path.join("subfolder"));
        app.select(subfolder.clone(), &egui::Context::default());
        assert!(app.expanded.contains(&Node::Computer));
        assert!(app.expanded.contains(&Node::Dir(path.clone())));
        assert!(!app.expanded.contains(&subfolder));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn archive_navigation_reveals_parent_branches_and_preserves_other_branches() {
        let root = PathBuf::from("library");
        let archive = root.join("books").join("book.cbz");
        let sibling = Node::Dir(root.join("photos"));
        let mut app = App::new(None);
        app.expanded.insert(sibling.clone());
        app.reveal_ancestors(&Node::ArchiveDir {
            archive: archive.clone(), prefix: "chapter/pages/".into(),
        });
        assert!(app.expanded.contains(&Node::Computer));
        assert!(app.expanded.contains(&Node::Dir(root.clone())));
        assert!(app.expanded.contains(&Node::Dir(root.join("books"))));
        assert!(app.expanded.contains(&Node::Archive(archive.clone())));
        assert!(app.expanded.contains(&Node::ArchiveDir { archive, prefix: "chapter/".into() }));
        assert!(app.expanded.contains(&sibling));
    }
}





