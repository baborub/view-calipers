//! Named canvas library with owned imports and references to unchanged original files.
use super::*;
use std::sync::OnceLock;
pub mod canvas;
pub mod storage;
#[cfg(test)]
mod tests_canvas;
#[derive(Clone, Default)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub paths: Vec<PathBuf>,
    pub folder: String,
}
#[derive(Clone, Default)]
pub struct Library {
    pub revision: u64,
    pub collections: Vec<Collection>,
}
pub fn path() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!(
            "view-calipers-collections-test-{}.tsv",
            std::process::id()
        ))
    } else {
        settings::Settings::path().with_file_name("collections.tsv")
    }
}
fn hex(text: &str) -> String {
    text.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(text: &str) -> Option<String> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    String::from_utf8(
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
            .collect::<Option<Vec<_>>>()?,
    )
    .ok()
}
fn identity(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}
impl Library {
    fn parse(text: &str) -> Result<Self> {
        let mut lib = Self::default();
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 3 {
                anyhow::bail!("Invalid collection record");
            }
            let id = fields[1];
            let value = unhex(fields[2]).context("Invalid collection text")?;
            match fields[0] {
                "C" => {
                    if lib.collections.iter().any(|c| c.id == id) {
                        anyhow::bail!("Duplicate collection ID");
                    }
                    lib.collections.push(Collection {
                        id: id.into(),
                        name: value,
                        paths: Vec::new(),
                        folder: String::new(),
                    });
                }
                "D" => {
                    fileops::valid_name(&value)?;
                    lib.collections
                        .iter_mut()
                        .find(|c| c.id == id)
                        .context("Unknown collection ID")?
                        .folder = value;
                }
                "F" => lib
                    .collections
                    .iter_mut()
                    .find(|c| c.id == id)
                    .context("Unknown collection ID")?
                    .paths
                    .push(PathBuf::from(value)),
                _ => anyhow::bail!("Unknown collection record"),
            }
        }
        Ok(lib)
    }
    fn text(&self) -> String {
        let mut text = String::from("# view-calipers collections v1\n");
        for c in &self.collections {
            text.push_str(&format!("C\t{}\t{}\n", c.id, hex(&c.name)));
            if !c.folder.is_empty() {
                text.push_str(&format!("D\t{}\t{}\n", c.id, hex(&c.folder)));
            }
            for file in &c.paths {
                text.push_str(&format!("F\t{}\t{}\n", c.id, hex(&file.to_string_lossy())));
            }
        }
        text
    }
    fn add(&mut self, id: &str, paths: &[PathBuf]) -> Result<()> {
        let c = self
            .collections
            .iter_mut()
            .find(|c| c.id == id)
            .context("Collection no longer exists")?;
        let mut known: HashSet<_> = c.paths.iter().map(|p| identity(p)).collect();
        for p in paths {
            if p.is_absolute() && known.insert(identity(p)) {
                c.paths.push(p.clone());
            }
        }
        Ok(())
    }
    fn reorder(&mut self, id: &str, paths: &[PathBuf], target: &Path, after: bool) -> Result<()> {
        let c = self
            .collections
            .iter_mut()
            .find(|c| c.id == id)
            .context("Collection no longer exists")?;
        let selected: HashSet<_> = paths.iter().map(|p| identity(p)).collect();
        if selected.contains(&identity(target)) {
            return Ok(());
        }
        let group: Vec<_> = c
            .paths
            .iter()
            .filter(|p| selected.contains(&identity(p)))
            .cloned()
            .collect();
        if group.len() != paths.len() {
            return self.add(id, paths);
        }
        c.paths.retain(|p| !selected.contains(&identity(p)));
        let index = c
            .paths
            .iter()
            .position(|p| identity(p) == identity(target))
            .map(|i| i + usize::from(after))
            .unwrap_or(c.paths.len());
        c.paths.splice(index..index, group);
        Ok(())
    }
}
struct Store {
    root: PathBuf,
    lib: Library,
    error: Option<String>,
}
// Tests that mutate the process-wide library must not relocate it while another
// test is browsing or importing into it. Keep this separate from the store lock
// so background import workers can still access the library during a test.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(storage::initialize()))
}

pub fn revision() -> u64 {
    store().lock().unwrap().lib.revision
}
pub fn list() -> Vec<(String, String, usize)> {
    store()
        .lock()
        .unwrap()
        .lib
        .collections
        .iter()
        .map(|c| (c.id.clone(), c.name.clone(), c.paths.len()))
        .collect()
}
pub fn get(id: &str) -> Option<Collection> {
    store()
        .lock()
        .unwrap()
        .lib
        .collections
        .iter()
        .find(|c| c.id == id)
        .cloned()
}
pub fn update(action: impl FnOnce(&mut Library) -> Result<()>) -> Result<()> {
    anyhow::ensure!(!storage::busy(), "Wait for the library copy to finish");
    let mut store = store().lock().unwrap();
    if let Some(error) = &store.error {
        anyhow::bail!("Collections could not be loaded: {error}");
    }
    let mut next = store.lib.clone();
    action(&mut next)?;
    let file = store.root.join("collections.tsv");
    std::fs::create_dir_all(file.parent().unwrap())?;
    let temporary = file.with_extension("tmp");
    std::fs::write(&temporary, next.text())?;
    std::fs::rename(temporary, file)?;
    next.revision = store.lib.revision + 1;
    store.lib = next;
    Ok(())
}
pub fn create(name: &str, paths: &[PathBuf]) -> Result<String> {
    let name = name.trim();
    if fileops::valid_name(name).is_err() {
        anyhow::bail!("Enter a collection name");
    }
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let id = format!(
        "{:x}-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        SERIAL.fetch_add(1, AtomicOrdering::Relaxed)
    );
    let root = storage::root();
    update(|lib| {
        lib.collections.push(Collection {
            id: id.clone(),
            name: name.into(),
            paths: Vec::new(),
            folder: name.into(),
        });
        lib.add(&id, paths)?;
        storage::prepare(lib.collections.last().unwrap(), &root)
    })?;
    Ok(id)
}
pub fn add(id: &str, paths: &[PathBuf]) -> Result<()> {
    let old = get(id).context("Collection missing")?;
    let added: Vec<_> = paths
        .iter()
        .filter(|p| !old.paths.iter().any(|o| identity(o) == identity(p)))
        .cloned()
        .collect();
    let mut editor = board::Editor::open(&storage::board_path(id)?)?;
    editor.add_files(&added, None);
    editor.flush()?;
    update(|lib| lib.add(id, paths))
}
pub fn remove(id: &str, paths: &[PathBuf]) -> Result<()> {
    let selected: HashSet<_> = paths.iter().map(|p| identity(p)).collect();
    update(|lib| {
        lib.collections
            .iter_mut()
            .find(|c| c.id == id)
            .context("Collection no longer exists")?
            .paths
            .retain(|p| !selected.contains(&identity(p)));
        Ok(())
    })
}
pub fn reorder(id: &str, paths: &[PathBuf], target: &Path, after: bool) -> Result<()> {
    update(|lib| lib.reorder(id, paths, target, after))
}

impl App {
    pub(super) fn collection_message(&mut self, result: Result<()>) {
        self.file_message = match result {
            Ok(()) => "Collection updated · original files unchanged".into(),
            Err(error) => error.to_string(),
        };
    }
    pub(super) fn collection_sidebar(&mut self, ui: &mut egui::Ui) -> Option<Node> {
        if storage::busy() {
            ui.label("Copying collection library…");
            return None;
        }
        ui.horizontal(|ui| {
            ui.strong("Collections");
            if ui
                .small_button("+")
                .on_hover_text("New collection canvas")
                .clicked()
            {
                self.files.collection_dialog = Some((None, String::new(), Vec::new()));
            }
        });
        if let Some(error) = storage::error() {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if list().is_empty() {
            ui.small("Create a collection, then drag images onto its canvas.");
        }
        for (id, name, _) in list() {
            let response =
                ui_theme::sidebar_label(ui, self.canvas.id.as_deref() == Some(&id), &name)
                    .on_hover_text(format!("{name}\nOpen canvas below the file preview"));
            if response.clicked() {
                self.open_collection_board(&id, ui.ctx());
            }
            if egui::DragAndDrop::has_payload_of_type::<board::ImageDrag>(ui.ctx()) {
                if let Some(payload) = response.dnd_release_payload::<board::ImageDrag>() {
                    self.open_collection_board(&id, ui.ctx());
                    if let Some(editor) = &mut self.canvas.editor {
                        editor.add_sources(&payload.sources, None);
                    }
                }
            } else if let Some(files) = response.dnd_release_payload::<crate::clipboard::Files>() {
                self.collection_message(add(&id, &files.paths));
                self.open_collection_board(&id, ui.ctx());
            }
            response.context_menu(|ui| {
                if ui.button("Rename collection…").clicked() {
                    self.files.collection_dialog =
                        Some((Some(id.clone()), name.clone(), Vec::new()));
                    ui.close_menu();
                }
                if ui.button("Show collection folder").clicked() {
                    if let Ok(path) = storage::board_path(&id) {
                        self.select(Node::Dir(path.parent().unwrap().into()), ui.ctx());
                    }
                    ui.close_menu();
                }
                if ui.button("Remove from library (keep folder)").clicked() {
                    if self.canvas.id.as_deref() != Some(&id) || self.clear_collection_canvas() {
                        self.collection_message(update(|lib| {
                            lib.collections.retain(|c| c.id != id);
                            Ok(())
                        }));
                    }
                    ui.close_menu();
                }
            });
        }
        let size = Vec2::new(
            ui.available_width(),
            (ui.clip_rect().bottom() - ui.cursor().top()).max(45.0),
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        #[cfg(test)]
        {
            self.canvas.blank = Some(rect);
        }
        let _ = rect;
        if response.clicked() {
            self.clear_collection_canvas();
        }
        None
    }
    pub(super) fn collection_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, name, paths)) = &mut self.files.collection_dialog else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(if id.is_some() {
            "Rename collection"
        } else {
            "New collection"
        })
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Collection name");
            ui.text_edit_singleline(name);
            ui.small("Creates a canvas and its image folder in your collection library.");
            ui.horizontal(|ui| {
                apply = ui
                    .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        let _ = paths;
        apply |= ctx.input(|i| i.key_pressed(Key::Enter));
        cancel |= ctx.input(|i| i.key_pressed(Key::Escape));
        if cancel {
            self.files.collection_dialog = None;
            self.files.new_board_sources.clear();
        } else if apply && !name.trim().is_empty() {
            let (id, name, paths) = self.files.collection_dialog.take().unwrap();
            let result = if let Some(id) = id {
                let reopen = self.canvas.id.as_deref() == Some(&id);
                if board::imports_running() {
                    self.file_message = "Wait for image imports to finish.".into();
                    return;
                }
                if reopen && !self.clear_collection_canvas() {
                    return;
                }
                let result = storage::rename(&id, &name);
                if reopen {
                    self.open_collection_board(&id, ctx);
                }
                result
            } else {
                create(&name, &paths).map(|id| {
                    self.open_collection_board(&id, ctx);
                    let sources = std::mem::take(&mut self.files.new_board_sources);
                    if let Some(editor) = &mut self.canvas.editor {
                        if !sources.is_empty() {
                            editor.add_sources(&sources, None);
                        }
                        editor.fit();
                    }
                })
            };
            self.collection_message(result);
        }
    }
    pub(super) fn poll_collection(&mut self, ctx: &egui::Context) {
        if self.files.search.results {
            return;
        }
        if let Some(Node::Collection { id, .. }) = self.selected.clone() {
            if self.files.collection_revision != revision() {
                self.files.collection_revision = revision();
                if let Some(c) = get(&id) {
                    for node in &mut self.history {
                        if let Node::Collection { id: old, name } = node {
                            if old == &id {
                                *name = c.name.clone();
                            }
                        }
                    }
                    self.tabs[self.active_tab].title = c.name.clone();
                    self.selected = Some(Node::Collection {
                        id: id.clone(),
                        name: c.name,
                    });
                    if self.files.collection_order == c.paths {
                        return;
                    }
                }
                let locked = self.tabs[self.active_tab].locked;
                self.tabs[self.active_tab].locked = false;
                let selected = self.selected_paths();
                let current = self.current.and_then(|i| self.images.get(i)).and_then(|r| {
                    if let ImageSrc::File(p) = &r.src {
                        Some(p.clone())
                    } else {
                        None
                    }
                });
                if let Some(c) = get(&id) {
                    for node in &mut self.history {
                        if let Node::Collection { id: old, name } = node {
                            if old == &id {
                                *name = c.name.clone();
                            }
                        }
                    }
                    self.select(Node::Collection { id, name: c.name }, ctx);
                    for (i, row) in self.files.rows.iter().enumerate() {
                        if row.path.as_ref().is_some_and(|p| selected.contains(p)) {
                            self.files.selected.insert(i);
                        }
                    }
                    if let Some(index) = current.and_then(|p| {
                        self.images
                            .iter()
                            .position(|r| matches!(&r.src,ImageSrc::File(path) if path==&p))
                    }) {
                        self.goto(index, ctx);
                    }
                } else {
                    self.select(Node::Computer, ctx);
                }
                self.tabs[self.active_tab].locked = locked;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_references_roundtrip_deduplicate_and_keep_custom_order() {
        let mut lib = Library::default();
        lib.collections.push(Collection {
            id: "test".into(),
            name: "Свет / Lighting".into(),
            folder: String::new(),
            paths: Vec::new(),
        });
        let files: Vec<_> = [
            "C:/refs/one.jpg",
            "C:/refs/two.mp4",
            "D:/Модели/model.glb",
            "C:/docs/reference.pdf",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        lib.add("test", &files).unwrap();
        lib.add("test", &[PathBuf::from("c:\\REFS\\ONE.JPG")])
            .unwrap();
        assert_eq!(lib.collections[0].paths.len(), 4);
        lib.reorder("test", &files[0..2], &files[3], true).unwrap();
        assert_eq!(
            lib.collections[0].paths,
            vec![
                files[2].clone(),
                files[3].clone(),
                files[0].clone(),
                files[1].clone()
            ]
        );
        let text = lib.text();
        let loaded = Library::parse(&text).unwrap();
        assert_eq!(loaded.collections[0].name, "Свет / Lighting");
        assert_eq!(loaded.collections[0].paths, lib.collections[0].paths);
        assert!(Library::parse("F\tunknown\t6162").is_err());
    }
    #[test]
    fn collections_browse_filter_remove_and_refresh_locked_tabs_without_touching_sources() {
        let _library = test_guard();
        let ctx = egui::Context::default();
        let root = std::env::temp_dir().join(format!(
            "view-calipers-collection-fixtures-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.png");
        let b = root.join("b.png");
        let document = root.join("readme.md");
        let missing = root.join("missing.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]))
            .save(&a)
            .unwrap();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255]))
            .save(&b)
            .unwrap();
        std::fs::write(&document, "# Reference").unwrap();
        let refs = vec![b.clone(), document.clone(), a.clone(), missing.clone()];
        let id = create("Test collection", &refs).unwrap();
        let mut app = App::new(None);
        app.select(
            Node::Collection {
                id: id.clone(),
                name: "Test collection".into(),
            },
            &ctx,
        );
        assert_eq!(
            app.files
                .rows
                .iter()
                .filter_map(|r| r.path.clone())
                .collect::<Vec<_>>(),
            refs
        );
        assert_eq!(app.media_navigation(), vec![0, 1, 2]);
        assert!(app.files.rows[3].label.contains("[missing]"));
        app.reveal_original(&a, &ctx);
        assert_eq!(app.selected, Some(Node::Dir(root.clone())));
        assert!(app
            .files
            .selected
            .iter()
            .any(|i| app.files.rows[*i].path.as_ref() == Some(&a)));
        app.select(
            Node::Collection {
                id: id.clone(),
                name: "Test collection".into(),
            },
            &ctx,
        );
        app.files.search.query = "ext:png".into();
        app.apply_file_filter();
        assert_eq!(app.media_navigation(), vec![0, 2]);
        app.files.search.query.clear();
        app.apply_file_filter();
        let row = app
            .files
            .rows
            .iter()
            .position(|r| r.path.as_ref() == Some(&a))
            .unwrap();
        app.files.select_only(row);
        app.files.pending = Some(file_browser::Command::Delete);
        app.file_commands(&ctx);
        assert!(a.exists());
        assert!(app.workbench.recycle.is_none());
        app.poll_collection(&ctx);
        assert!(!app.files.rows.iter().any(|r| r.path.as_ref() == Some(&a)));
        let unrelated = create("Unrelated", &[]).unwrap();
        app.status = "Keep current preview".into();
        app.poll_collection(&ctx);
        assert_eq!(app.status, "Keep current preview");
        update(|lib| {
            lib.collections.retain(|c| c.id != unrelated);
            Ok(())
        })
        .unwrap();
        app.poll_collection(&ctx);
        app.files.collection_custom = false;
        app.tabs[app.active_tab].locked = true;
        let count = app.tabs.len();
        update(|lib| {
            lib.collections
                .iter_mut()
                .find(|c| c.id == id)
                .unwrap()
                .name = "Renamed".into();
            Ok(())
        })
        .unwrap();
        app.poll_collection(&ctx);
        assert_eq!(app.tabs.len(), count);
        assert_eq!(app.selected.as_ref().unwrap().label(), "Renamed");
        assert!(app.tabs[app.active_tab].locked);
        assert!(!app.files.collection_custom);
        app.select(
            Node::Collection {
                id: id.clone(),
                name: "Old title".into(),
            },
            &ctx,
        );
        assert_eq!(app.selected.as_ref().unwrap().label(), "Renamed");
        update(|lib| {
            lib.collections.retain(|c| c.id != id);
            Ok(())
        })
        .unwrap();
        app.poll_collection(&ctx);
        assert!(a.exists() && b.exists() && document.exists());
        assert_eq!(app.selected, Some(Node::Computer));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        sidebar: bool,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if sidebar {
                        app.collection_sidebar(ui);
                    } else {
                        app.file_gallery(ui);
                    }
                });
            },
        )
    }
    fn point(output: &egui::FullOutput, label: &str) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|s| {
                if let egui::epaint::Shape::Text(t) = &s.shape {
                    (t.galley.job.text == label).then(|| t.pos + t.galley.size() * 0.5)
                } else {
                    None
                }
            })
            .unwrap()
    }
    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn group_drag_reorders_collection_and_sidebar_drop_adds_only_references() {
        let _library = test_guard();
        let root = std::env::temp_dir().join(format!(
            "view-calipers-collection-drag-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let files: Vec<_> = ["a.txt", "b.txt", "c.txt"]
            .into_iter()
            .map(|n| root.join(n))
            .collect();
        for file in &files {
            std::fs::write(file, b"original").unwrap();
        }
        let id = create("Drag collection", &files).unwrap();
        let mut app = App::new(None);
        let ctx = egui::Context::default();
        app.select(
            Node::Collection {
                id: id.clone(),
                name: "Drag collection".into(),
            },
            &ctx,
        );
        app.files.details = true;
        app.files.selected = HashSet::from([0, 1]);
        let output = frame(&mut app, &ctx, vec![], false);
        let a = point(&output, "a.txt");
        let c = point(&output, "c.txt") + Vec2::new(0.0, 5.0);
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(a), button(a, true)],
            false,
        );
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(a + Vec2::new(20.0, 0.0))],
            false,
        );
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(c)], false);
        frame(&mut app, &ctx, vec![button(c, false)], false);
        assert_eq!(
            get(&id).unwrap().paths,
            vec![files[2].clone(), files[0].clone(), files[1].clone()]
        );
        let empty = create("Drop target", &[]).unwrap();
        let output = frame(&mut app, &ctx, vec![], true);
        let target = point(&output, "Drop target");
        egui::DragAndDrop::set_payload(
            &ctx,
            crate::clipboard::Files {
                paths: files.clone(),
                cut: true,
            },
        );
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(target), button(target, true)],
            true,
        );
        frame(&mut app, &ctx, vec![button(target, false)], true);
        assert_eq!(get(&empty).unwrap().paths, files);
        for file in &files {
            assert_eq!(std::fs::read(file).unwrap(), b"original");
        }
        update(|lib| {
            lib.collections.retain(|c| c.id != id && c.id != empty);
            Ok(())
        })
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
