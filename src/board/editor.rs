use super::operations::Command;
use super::*;
use egui::{PointerButton, Stroke};
mod tools;

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Hand,
    Text,
    Draw,
    Crop,
}
enum Gesture {
    Pan {
        start: Pos2,
        camera: Camera,
    },
    Marquee {
        start: Pos2,
        base: HashSet<u64>,
    },
    Move {
        start: Pos2,
        objects: Vec<Object>,
    },
    Scale {
        start: Pos2,
        pivot: Pos2,
        objects: Vec<Object>,
    },
    Rotate {
        start: Pos2,
        pivot: Pos2,
        objects: Vec<Object>,
    },
    Draw(Vec<Pos2>),
    Crop {
        start: Pos2,
        end: Pos2,
        before: Object,
        texture: [usize; 2],
    },
}
pub struct Editor {
    collection: bool,
    importing: Arc<std::sync::atomic::AtomicUsize>,
    import_message: Arc<Mutex<String>>,
    shared: Arc<Mutex<Shared>>,
    assets: assets::Assets,
    media: media::Media,
    id: u64,
    selected: HashSet<u64>,
    controls_id: Option<u64>,
    controls_height: f32,
    tool: Tool,
    gesture: Option<Gesture>,
    edit_text: Option<(u64, String)>,
    pub focused: bool,
    pub area: Rect,
    grid: bool,
    need_fit: bool,
    color: [u8; 4],
    pen_width: f32,
    message: String,
    items_open: bool,
    focus_camera: Option<Camera>,
    isolate: bool,
    isolated_ids: HashSet<u64>,
    media_sizes: HashMap<u64, [usize; 2]>,
    grayscale: bool,
    context_target: Option<u64>,
}
impl Drop for Editor {
    fn drop(&mut self) {
        let mut s = self.shared.lock().unwrap();
        if s.gesture == Some(self.id) {
            s.gesture = None;
        }
        let _ = s.flush();
    }
}
impl Editor {
    pub fn open(path: &Path) -> Result<Self> {
        static ID: AtomicU64 = AtomicU64::new(1);
        Ok(Self {
            collection: false,
            importing: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            import_message: Arc::new(Mutex::new(String::new())),
            shared: open_shared(path)?,
            assets: assets::Assets::default(),
            media: media::Media::default(),
            id: ID.fetch_add(1, AtomicOrdering::Relaxed),
            selected: HashSet::new(),
            controls_id: None,
            controls_height: 0.0,
            tool: Tool::Select,
            gesture: None,
            edit_text: None,
            focused: false,
            area: Rect::NOTHING,
            grid: false,
            need_fit: false,
            color: [255, 203, 105, 255],
            pen_width: 3.0,
            message: String::new(),
            items_open: false,
            focus_camera: None,
            isolate: false,
            isolated_ids: HashSet::new(),
            media_sizes: HashMap::new(),
            grayscale: false,
            context_target: None,
        })
    }
    pub fn path(&self) -> PathBuf {
        self.shared.lock().unwrap().path.clone()
    }
    pub fn set_collection(&mut self, collection: bool) {
        self.collection = collection;
    }
    pub fn importing(&self) -> bool {
        self.importing.load(AtomicOrdering::Relaxed) > 0
    }
    fn copy_selection_into_folder(&mut self, ctx: &egui::Context) {
        if self.importing() {
            return;
        }
        let shared = self.shared.clone();
        let s = shared.lock().unwrap();
        let path = s.path.clone();
        let selected: Vec<_> = s
            .doc
            .objects
            .iter()
            .filter(|o| {
                self.selected.contains(&o.id)
                    && (o.content.image_source(&path).is_some()
                        || matches!(o.content, Content::Video { .. } | Content::Markdown { .. }))
            })
            .map(|o| (o.id, o.content.clone()))
            .collect();
        drop(s);
        if selected.is_empty() {
            return;
        }
        let count = self.importing.clone();
        count.fetch_add(1, AtomicOrdering::Relaxed);
        IMPORTS.fetch_add(1, AtomicOrdering::Relaxed);
        let status = self.import_message.clone();
        *status.lock().unwrap() = "Copying files into the collection folder…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<usize> {
                let mut copied = Vec::new();
                for (id, content) in selected {
                    let target = match &content {
                        Content::Video { source } | Content::Markdown { source } => {
                            media::copy_file(&resolve(source, &path), path.parent().unwrap())?
                        }
                        _ => {
                            let source = content.image_source(&path).unwrap();
                            let (name, bytes) = match source {
                                ImageSrc::File(p) => {
                                    anyhow::ensure!(
                                        std::fs::metadata(&p)?.len() <= 64 * 1024 * 1024,
                                        "Image exceeds 64 MiB import limit"
                                    );
                                    (file_name(&p), std::fs::read(p)?)
                                }
                                ImageSrc::Entry { archive, name } => (
                                    file_name(Path::new(&name)),
                                    read_archive_entry(&archive, &name)?,
                                ),
                            };
                            crate::external_images::save_image(
                                path.parent().unwrap(),
                                &name,
                                &bytes,
                            )?
                        }
                    };
                    copied.push((id, content, target));
                }
                loop {
                    let mut s = shared.lock().unwrap();
                    if s.gesture.is_some() {
                        drop(s);
                        std::thread::sleep(Duration::from_millis(40));
                        continue;
                    }
                    s.checkpoint();
                    let mut changed = 0;
                    for (id, before, target) in copied {
                        if let Some(o) = s
                            .doc
                            .objects
                            .iter_mut()
                            .find(|o| o.id == id && o.content == before)
                        {
                            let source = reference(&target, &path);
                            o.content = match before {
                                Content::Video { .. } => Content::Video { source },
                                Content::Markdown { .. } => Content::Markdown { source },
                                _ => Content::Image { source },
                            };
                            changed += 1;
                        }
                    }
                    s.changed();
                    s.flush()?;
                    return Ok(changed);
                }
            })();
            *status.lock().unwrap() = match result {
                Ok(n) => format!("Copied {n} file(s); canvas layout preserved"),
                Err(e) => format!("Copy failed: {e}"),
            };
            count.fetch_sub(1, AtomicOrdering::Relaxed);
            IMPORTS.fetch_sub(1, AtomicOrdering::Relaxed);
            ctx.request_repaint();
        });
    }
    pub fn import_images(
        &mut self,
        input: crate::external_images::Input,
        point: Option<Pos2>,
        ctx: &egui::Context,
    ) {
        if self.importing.load(AtomicOrdering::Relaxed) >= 4 {
            self.message = "Wait for the current image imports to finish.".into();
            return;
        }
        let shared = self.shared.clone();
        let s = shared.lock().unwrap();
        if s.doc.canvas.locked {
            self.message = "Unlock the canvas in View before adding references.".into();
            return;
        }
        let directory = s.path.parent().unwrap().to_owned();
        let origin = point
            .map(|p| s.doc.camera.world(p, self.area))
            .unwrap_or(Pos2::new(s.doc.camera.center[0], s.doc.camera.center[1]));
        drop(s);
        let count = self.importing.clone();
        count.fetch_add(1, AtomicOrdering::Relaxed);
        IMPORTS.fetch_add(1, AtomicOrdering::Relaxed);
        let status = self.import_message.clone();
        *status.lock().unwrap() = "Importing image…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::external_images::import(input, &directory).and_then(|paths| loop {
                let mut s = shared.lock().unwrap();
                if s.gesture.is_some() {
                    drop(s);
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                s.checkpoint();
                let path = s.path.clone();
                let added = s.doc.add_images(&paths, &path, origin).len();
                s.changed();
                s.flush()?;
                return Ok(added);
            });
            *status.lock().unwrap() = match result {
                Ok(n) => format!("Imported {n} image(s) into the collection folder"),
                Err(e) => format!("Import failed: {e}"),
            };
            count.fetch_sub(1, AtomicOrdering::Relaxed);
            IMPORTS.fetch_sub(1, AtomicOrdering::Relaxed);
            ctx.request_repaint();
        });
    }
    pub fn fit(&mut self) {
        self.need_fit = true;
    }
    pub fn flush(&self) -> Result<()> {
        self.shared.lock().unwrap().flush()
    }
    pub fn update_focus(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !self.area.contains(p))
        }) {
            self.focused = false;
        }
        if self.edit_text.is_some() {
            self.focused = true;
        }
    }
    pub fn owns_keyboard(&self) -> bool {
        self.focused || self.edit_text.is_some() || self.media.reader_open()
    }
    pub fn add_files(&mut self, paths: &[PathBuf], point: Option<Pos2>) {
        self.add_sources(
            &paths
                .iter()
                .cloned()
                .map(ImageSrc::File)
                .collect::<Vec<_>>(),
            point,
        );
    }
    pub fn add_sources(&mut self, sources: &[ImageSrc], point: Option<Pos2>) {
        let mut s = self.shared.lock().unwrap();
        if s.doc.canvas.locked {
            self.message = "Unlock the canvas in View before adding references.".into();
            return;
        }
        if s.gesture.is_some() {
            return;
        }
        let origin = point
            .map(|p| s.doc.camera.world(p, self.area))
            .unwrap_or(Pos2::new(s.doc.camera.center[0], s.doc.camera.center[1]));
        let path = s.path.clone();
        s.checkpoint();
        self.selected = s
            .doc
            .add_sources(sources, &path, origin)
            .into_iter()
            .collect();
        self.message = if self.selected.is_empty() {
            "Choose images, video or Markdown files for the canvas.".into()
        } else {
            format!("Added {} media references", self.selected.len())
        };
        if !self.selected.is_empty() {
            s.changed();
        }
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.media.pause_when_hidden(ctx);
        let mut s = self.shared.lock().unwrap();
        if s.dirty && s.error.is_none() {
            if Instant::now() >= s.due && s.gesture.is_none() {
                let _ = s.flush();
            } else {
                ctx.request_repaint_after(
                    s.due
                        .saturating_duration_since(Instant::now())
                        .max(Duration::from_millis(100)),
                );
            }
        }
    }
    fn selection_bounds(&self, doc: &Document) -> Option<Rect> {
        doc.objects
            .iter()
            .filter(|o| self.selected.contains(&o.id))
            .map(Object::bounds)
            .reduce(Rect::union)
    }
    fn fit_document(&mut self, doc: &mut Document, area: Rect) {
        let bounds = self
            .selection_bounds(doc)
            .or_else(|| doc.objects.iter().map(Object::bounds).reduce(Rect::union));
        if let Some(bounds) = bounds {
            doc.camera.center = [bounds.center().x, bounds.center().y];
            doc.camera.zoom = ((area.width() - 80.0).max(20.0) / bounds.width().max(1.0))
                .min((area.height() - 80.0).max(20.0) / bounds.height().max(1.0))
                .clamp(0.005, 4.0);
        }
    }
    fn delete_selected(&mut self, s: &mut Shared) {
        if self.selected.is_empty() || s.doc.canvas.locked {
            return;
        }
        s.checkpoint();
        s.doc
            .objects
            .retain(|o| !self.selected.contains(&o.id) || o.options.locked);
        s.doc.clean_groups();
        self.selected.clear();
        s.changed();
    }
    fn transform(&self, s: &mut Shared, scale: f32, angle: f32) {
        if s.doc.canvas.locked {
            return;
        }
        let Some(bounds) = self.selection_bounds(&s.doc) else {
            return;
        };
        let objects: Vec<_> = s
            .doc
            .objects
            .iter()
            .filter(|o| self.selected.contains(&o.id) && !o.options.locked)
            .cloned()
            .collect();
        s.checkpoint();
        apply_transform(
            &mut s.doc,
            &objects,
            bounds.center(),
            scale,
            angle,
            Vec2::ZERO,
        );
        s.changed();
    }
    fn note(&mut self, s: &mut Shared, point: Pos2) {
        if s.doc.objects.len() >= MAX_OBJECTS || s.doc.canvas.locked {
            return;
        }
        s.checkpoint();
        let id = s.doc.next_id();
        s.doc.objects.push(Object {
            id,
            center: [point.x, point.y],
            size: [280.0, 160.0],
            angle: 0.0,
            options: Default::default(),
            content: Content::Text {
                text: "New note".into(),
                color: self.color,
            },
        });
        self.selected = HashSet::from([id]);
        self.edit_text = Some((id, "New note".into()));
        self.tool = Tool::Select;
        s.changed();
    }
    fn save_as(&mut self) {
        let path = self.path();
        if let Some(target) = rfd::FileDialog::new()
            .add_filter("Reference board", &["vcboard"])
            .set_file_name(file_name(&path))
            .save_file()
        {
            if target == path {
                let _ = self.flush();
                return;
            }
            let mut doc = self.shared.lock().unwrap().doc.clone();
            for obj in &mut doc.objects {
                if let Some(source) = obj.content.source_path_mut() {
                    *source = reference(&resolve(source, &path), &target);
                }
            }
            let result = write(&target, &doc).and_then(|_| open_shared(&target));
            match result {
                Ok(shared) => {
                    self.shared = shared;
                    self.message = "Saved board copy".into();
                }
                Err(e) => self.message = e.to_string(),
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let paste = self.owns_keyboard()
            && !ui.ctx().wants_keyboard_input()
            && ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL, Key::V)
                    || i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
            });
        if paste {
            match crate::external_images::clipboard() {
                Ok(crate::external_images::Input::Files(paths)) => self.add_files(&paths, None),
                Ok(input) => self.import_images(input, None, ui.ctx()),
                Err(e) => self.message = format!("Paste: {e}"),
            }
        }
        self.tick(ui.ctx());
        self.assets.begin(ui.ctx());
        self.media.begin(ui.ctx());
        let shared = self.shared.clone();
        let mut s = shared.lock().unwrap();
        let mut fullscreen = false;
        let mut save_as = false;
        let mut add = false;
        let mut relink = false;
        let mut copy_into_folder = false;
        let mut toolbar = Rect::NOTHING;
        let busy = s.gesture.is_some_and(|owner| owner != self.id);
        let header = ui.horizontal_wrapped(|ui| {
            ui.strong(if self.collection {
                &s.doc.title
            } else {
                "Reference Board"
            });
            if ui.small_button("Fullscreen").clicked() {
                fullscreen = true;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Fit").small())
                .on_hover_text("Fit selected objects, or the whole board · F")
                .clicked()
            {
                self.need_fit = true;
            }
            if ui.selectable_label(self.grid, "Overview").clicked() {
                self.grid = !self.grid;
            }
            ui.menu_button("Board", |ui| {
                ui.label(&s.doc.title);
                if ui.button("Add files…").clicked() {
                    add = true;
                    ui.close_menu();
                }
                if ui.button("Save · Ctrl+S").clicked() {
                    let _ = s.flush();
                    ui.close_menu();
                }
                if !self.collection && ui.button("Save as…").clicked() {
                    save_as = true;
                    ui.close_menu();
                }
                if ui.button("Reload media previews").clicked() {
                    self.assets = assets::Assets::default();
                    self.media = media::Media::default();
                    ui.close_menu();
                }
                ui.separator();
                ui.label("Wheel: zoom · Middle drag / Space+drag: pan");
                ui.label("Shift+click: multi-select · Drag empty space: select box");
                ui.label("Corner handle: scale · Round handle: rotate");
                ui.label("V: select · H: pan · T: text · B: draw");
                ui.label("Delete: remove objects · Ctrl+Z / Ctrl+Y: undo / redo");
                ui.label("Double-click an image/background: fullscreen");
                ui.label("Double-click: note → edit, video → play/pause, Markdown → read");
                ui.small(s.path.display().to_string());
            });
            ui.menu_button("View", |ui| self.reference_view(ui, &mut s));
            ui.small(if busy {
                "Editing in another preview"
            } else if s.dirty {
                "Unsaved"
            } else {
                "Saved"
            });
        });
        toolbar = toolbar.union(header.response.rect);
        let import_message = self.import_message.lock().unwrap().clone();
        if !import_message.is_empty() {
            ui.small(import_message);
        }
        let tools = ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (tool, label) in [
                    (Tool::Select, "Select"),
                    (Tool::Hand, "Pan"),
                    (Tool::Text, "Text"),
                    (Tool::Draw, "Draw"),
                    (Tool::Crop, "Crop"),
                ] {
                    ui.selectable_value(&mut self.tool, tool, label);
                }
                ui.color_edit_button_srgba_unmultiplied(&mut self.color);
                if self.tool == Tool::Draw {
                    ui.add(
                        egui::DragValue::new(&mut self.pen_width)
                            .range(1.0..=30.0)
                            .suffix(" px"),
                    );
                }
                if ui
                    .add_enabled(!s.undo.is_empty(), egui::Button::new("Undo"))
                    .clicked()
                {
                    s.undo(false);
                }
                if ui
                    .add_enabled(!s.redo.is_empty(), egui::Button::new("Redo"))
                    .clicked()
                {
                    s.undo(true);
                }
                ui.menu_button("Selection", |ui| {
                    self.reference_actions(ui, &mut s);
                    ui.separator();
                    if ui.button("Select all").clicked() {
                        self.selected = s.doc.objects.iter().map(|o| o.id).collect();
                        ui.close_menu();
                    }
                    if ui.button("Clear selection").clicked() {
                        self.selected.clear();
                        ui.close_menu();
                    }
                    ui.add_enabled_ui(!self.selected.is_empty(), |ui| {
                        if ui.button("Rotate -15°").clicked() {
                            self.transform(&mut s, 1.0, -15_f32.to_radians());
                        }
                        if ui.button("Rotate +15°").clicked() {
                            self.transform(&mut s, 1.0, 15_f32.to_radians());
                        }
                        if ui.button("Scale 90%").clicked() {
                            self.transform(&mut s, 0.9, 0.0);
                        }
                        if ui.button("Scale 110%").clicked() {
                            self.transform(&mut s, 1.1, 0.0);
                        }
                        if ui.button("Bring to front").clicked() {
                            self.command(&mut s, Command::Front);
                        }
                        if ui.button("Send to back").clicked() {
                            self.command(&mut s, Command::Back);
                        }
                        if ui.button("Remove objects · Delete").clicked() {
                            self.delete_selected(&mut s);
                            ui.close_menu();
                        }
                        if self.selected.len() == 1 && ui.button("Relink source…").clicked() {
                            relink = true;
                            ui.close_menu();
                        }
                        if self.collection
                            && ui
                                .add_enabled(
                                    !self.importing(),
                                    egui::Button::new("Copy files into collection folder"),
                                )
                                .clicked()
                        {
                            copy_into_folder = true;
                            ui.close_menu();
                        }
                    });
                });
                ui.menu_button("Arrange", |ui| self.arrange_menu(ui, &mut s));
            })
        });
        toolbar = toolbar.union(tools.response.rect);
        let controls_top = ui.cursor().top();
        // Keep the canvas origin fixed for the duration of a pointer gesture.
        if self.gesture.is_none() && !ui.input(|i| i.pointer.any_down()) {
            self.controls_id = (self.selected.len() == 1)
                .then(|| self.selected.iter().copied().next())
                .flatten();
        }
        if let Some(object) = s.doc.objects.iter().find(|o| {
            Some(o.id) == self.controls_id
                && matches!(o.content, Content::Video { .. } | Content::Markdown { .. })
        }) {
            let controls = ui.scope(|ui| self.media.controls(ui, object, &s.path));
            toolbar = toolbar.union(controls.response.rect);
        }
        let controls_height = ui.cursor().top() - controls_top;
        if self.area.is_positive() && !self.need_fit && controls_height != self.controls_height {
            // Revealing playback controls must not move the object away from the second click.
            s.doc.camera.center[1] +=
                (controls_height - self.controls_height) * 0.5 / s.doc.camera.zoom;
            s.changed();
        }
        self.controls_height = controls_height;
        if ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| toolbar.contains(p))
        }) {
            self.focused = true;
        }
        if let Some(error) = &s.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if !self.message.is_empty() {
            ui.add(egui::Label::new(RichText::new(&self.message).small()).truncate());
        }
        ui.separator();
        let area = ui.available_rect_before_wrap();
        self.area = area;
        ui.advance_cursor_after_rect(area);
        let response = ui.interact(
            area,
            ui.id().with(("board_canvas", self.id)),
            Sense::click_and_drag(),
        );
        let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
        painter.rect_filled(area, 0.0, Color32::from_rgb(17, 21, 28));
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let hovered = response.hovered();
        if response.clicked() || response.drag_started() {
            self.focused = true;
        }
        if self.need_fit && !busy && area.is_positive() {
            self.fit_document(&mut s.doc, area);
            self.need_fit = false;
            s.changed();
        }
        if !busy
            && self.focused
            && !ui.ctx().wants_keyboard_input()
            && self.edit_text.is_none()
            && !self.media.reader_open()
        {
            self.reference_shortcuts(ui, &mut s);
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::CTRL, Key::Y)
                    || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Z)
                {
                    s.undo(true);
                } else if i.consume_key(egui::Modifiers::CTRL, Key::Z) {
                    s.undo(false);
                }
                if i.consume_key(egui::Modifiers::CTRL, Key::S) {
                    let _ = s.flush();
                }
                if i.consume_key(egui::Modifiers::CTRL, Key::A) {
                    self.selected = s.doc.objects.iter().map(|o| o.id).collect();
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Delete) {
                    self.delete_selected(&mut s);
                }
                if i.consume_key(egui::Modifiers::NONE, Key::F) {
                    self.need_fit = true;
                }
                for (key, tool) in [
                    (Key::V, Tool::Select),
                    (Key::H, Tool::Hand),
                    (Key::T, Tool::Text),
                    (Key::B, Tool::Draw),
                    (Key::C, Tool::Crop),
                ] {
                    if i.consume_key(egui::Modifiers::NONE, key) {
                        self.tool = tool;
                    }
                }
            });
        }
        if response.secondary_clicked() {
            self.focused = true;
            self.context_target = response
                .interact_pointer_pos()
                .and_then(|p| self.hit(&s.doc, s.doc.camera.world(p, area), true));
            if let Some(id) = self.context_target {
                if !self.selected.contains(&id) {
                    self.selected = s.doc.selection_for(id, ui.input(|i| i.modifiers.alt));
                }
            }
        }
        response.context_menu(|ui| {
            self.reference_actions(ui, &mut s);
            let source = self
                .context_target
                .and_then(|id| s.doc.objects.iter().find(|o| o.id == id))
                .and_then(|o| match &o.content {
                    Content::Video { source } | Content::Markdown { source } => {
                        Some(ImageSrc::File(resolve(source, &s.path)))
                    }
                    content => content.image_source(&s.path),
                });
            if ui
                .add_enabled(source.is_some(), egui::Button::new("Open source folder"))
                .on_hover_text(
                    "Open the original location in the active file pane and select the source file",
                )
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("board_reveal_source"),
                        RevealSource {
                            source: source.unwrap(),
                            board: s.path.clone(),
                        },
                    )
                });
                ui.ctx().request_repaint();
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    !self.selected.is_empty(),
                    egui::Button::new("Remove from canvas · Delete"),
                )
                .clicked()
            {
                self.delete_selected(&mut s);
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button("Arrange", |ui| self.arrange_menu(ui, &mut s));
            ui.menu_button("View", |ui| self.reference_view(ui, &mut s));
        });
        if !busy && hovered && !self.grid && self.gesture.is_none() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                s.doc
                    .camera
                    .zoom_at(pointer.unwrap(), area, (scroll * 0.0025).exp());
                s.changed();
                ui.ctx().input_mut(|i| {
                    i.raw_scroll_delta = Vec2::ZERO;
                    i.smooth_scroll_delta = Vec2::ZERO;
                });
            }
        }
        // egui 0.29 takes the payload even when the requested type is wrong.
        if egui::DragAndDrop::has_payload_of_type::<ImageDrag>(ui.ctx()) {
            if let Some(payload) = response.dnd_release_payload::<ImageDrag>() {
                drop(s);
                self.add_sources(&payload.sources, pointer);
                s = shared.lock().unwrap();
            }
        } else if let Some(payload) = response.dnd_release_payload::<crate::clipboard::Files>() {
            drop(s);
            self.add_files(&payload.paths, pointer);
            s = shared.lock().unwrap();
        }
        if response
            .dnd_hover_payload::<crate::clipboard::Files>()
            .is_some()
            || response.dnd_hover_payload::<ImageDrag>().is_some()
        {
            painter.rect_stroke(
                area.shrink(3.0),
                0.0,
                Stroke::new(2.0_f32, Color32::LIGHT_BLUE),
            );
        }
        self.grayscale = s.doc.canvas.grayscale;
        if self.grid {
            self.show_grid(ui, &painter, &mut s, area);
        } else {
            if !busy {
                self.interact(ui, &response, &mut s, area, &mut fullscreen);
            }
            let camera = s.doc.camera;
            // Adaptive grid keeps a bounded number of lines even at extreme zoom levels.
            let spacing = s.doc.canvas.step * camera.zoom;
            let spacing = spacing * 2_f32.powf((24.0 / spacing).log2().ceil().max(0.0));
            let zero = camera.screen(Pos2::ZERO, area);
            let color = Color32::from_rgb(29, 35, 44);
            let mut x = area.left() + (zero.x - area.left()).rem_euclid(spacing);
            while s.doc.canvas.grid && x < area.right() {
                painter.line_segment(
                    [Pos2::new(x, area.top()), Pos2::new(x, area.bottom())],
                    Stroke::new(1.0_f32, color),
                );
                x += spacing;
            }
            let mut y = area.top() + (zero.y - area.top()).rem_euclid(spacing);
            while s.doc.canvas.grid && y < area.bottom() {
                painter.line_segment(
                    [Pos2::new(area.left(), y), Pos2::new(area.right(), y)],
                    Stroke::new(1.0_f32, color),
                );
                y += spacing;
            }
            let board_path = s.path.clone();
            self.paint_groups(&painter, &s.doc, area);
            for object in &s.doc.objects {
                if self.isolate && !self.isolated_ids.contains(&object.id) {
                    continue;
                }
                self.paint_object(ui, &painter, object, camera, area, &board_path);
            }
            if let Some(bounds) = self.selection_bounds(&s.doc).filter(|_| {
                !s.doc.canvas.locked
                    && s.doc
                        .objects
                        .iter()
                        .any(|o| self.selected.contains(&o.id) && !o.options.locked)
            }) {
                let r = Rect::from_min_max(
                    camera.screen(bounds.min, area),
                    camera.screen(bounds.max, area),
                );
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::LIGHT_BLUE));
                painter.rect_filled(
                    Rect::from_center_size(r.right_bottom(), Vec2::splat(10.0)),
                    2.0,
                    Color32::LIGHT_BLUE,
                );
                painter.line_segment(
                    [r.center_top(), r.center_top() - Vec2::new(0.0, 24.0)],
                    Stroke::new(1.0_f32, Color32::LIGHT_BLUE),
                );
                painter.circle_filled(
                    r.center_top() - Vec2::new(0.0, 24.0),
                    6.0,
                    Color32::LIGHT_BLUE,
                );
            }
            if let Some(Gesture::Marquee { start, .. }) = &self.gesture {
                if let Some(p) = pointer {
                    let rect = Rect::from_two_pos(camera.screen(*start, area), p);
                    painter.rect_filled(
                        rect,
                        0.0,
                        Color32::from_rgba_unmultiplied(100, 180, 250, 35),
                    );
                    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, Color32::LIGHT_BLUE));
                }
            }
            if let Some(Gesture::Draw(points)) = &self.gesture {
                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points.iter().map(|p| camera.screen(*p, area)).collect(),
                        Stroke::new(
                            self.pen_width,
                            Color32::from_rgba_unmultiplied(
                                self.color[0],
                                self.color[1],
                                self.color[2],
                                self.color[3],
                            ),
                        ),
                    ));
                }
            }
            self.paint_crop(&painter, camera, area);
            if s.doc.objects.is_empty() {
                painter.text(
                    area.center(),
                    Align2::CENTER_CENTER,
                    "Drop images, videos or Markdown here\nor use Board → Add files\n\nT: text note   B: draw",
                    FontId::proportional(16.0),
                    ui_theme::MUTED,
                );
            }
            painter.text(
                area.left_bottom() + Vec2::new(12.0, -12.0),
                Align2::LEFT_BOTTOM,
                format!(
                    "{} objects · {} selected · {:.0}%",
                    s.doc.objects.len(),
                    self.selected.len(),
                    camera.zoom * 100.0
                ),
                FontId::proportional(12.0),
                ui_theme::MUTED,
            );
        }
        self.assets.end(ui.ctx());
        self.media.end(&s.doc, &s.path);
        drop(s);
        if add {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter(
                    "Canvas files",
                    &IMAGE_EXT
                        .iter()
                        .chain(VIDEO_EXT)
                        .copied()
                        .chain(["md", "markdown"])
                        .collect::<Vec<_>>(),
                )
                .pick_files()
            {
                self.add_files(&paths, None);
            }
        }
        if copy_into_folder {
            self.copy_selection_into_folder(ui.ctx());
        }
        if relink {
            let archive = self.shared.lock().unwrap().doc.objects.iter().any(|o| {
                self.selected.contains(&o.id) && matches!(o.content, Content::ArchiveImage { .. })
            });
            let dialog = if archive {
                rfd::FileDialog::new().add_filter("Archive", ARCHIVE_EXT)
            } else {
                rfd::FileDialog::new().add_filter(
                    "Media",
                    &IMAGE_EXT
                        .iter()
                        .chain(VIDEO_EXT)
                        .copied()
                        .chain(["md", "markdown"])
                        .collect::<Vec<_>>(),
                )
            };
            if let Some(path) = dialog.pick_file() {
                let mut s = self.shared.lock().unwrap();
                s.checkpoint();
                let board = s.path.clone();
                if let Some(object) = s
                    .doc
                    .objects
                    .iter_mut()
                    .find(|o| self.selected.contains(&o.id))
                {
                    if let Some(source) = object.content.source_path_mut() {
                        *source = reference(&path, &board);
                        s.changed();
                        self.assets = assets::Assets::default();
                        self.media = media::Media::default();
                    }
                }
            }
        }
        if save_as {
            self.save_as();
        }
        self.text_window(ui.ctx());
        self.items_window(ui.ctx());
        self.media.reader_window(ui.ctx(), self.id);
        self.tick(ui.ctx());
        if self.need_fit {
            ui.ctx().request_repaint();
        }
        fullscreen
    }
    fn show_grid(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        s: &mut Shared,
        area: Rect,
    ) {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(area), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("board-grid", self.id))
                .show(ui, |ui| {
                    let columns = (area.width() / 150.0).floor().max(1.0) as usize;
                    for row in s.doc.objects.chunks(columns) {
                        ui.horizontal(|ui| {
                            for object in row {
                                let (r, response) =
                                    ui.allocate_exact_size(Vec2::new(142.0, 130.0), Sense::click());
                                let mut small = object.clone();
                                small.angle = 0.0;
                                small.center = [r.center().x, r.center().y];
                                let fit = (120.0 / small.size[0]).min(100.0 / small.size[1]);
                                small.size[0] *= fit;
                                small.size[1] *= fit;
                                let camera = Camera {
                                    center: [area.center().x, area.center().y],
                                    zoom: 1.0,
                                };
                                self.paint_object(
                                    ui,
                                    &painter.with_clip_rect(r.intersect(ui.clip_rect())),
                                    &small,
                                    camera,
                                    area,
                                    &s.path,
                                );
                                if response.clicked() {
                                    self.selected = HashSet::from([object.id]);
                                    self.grid = false;
                                    self.need_fit = true;
                                    self.focused = true;
                                }
                            }
                        });
                    }
                });
        });
    }
    fn paint_object(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        object: &Object,
        camera: Camera,
        area: Rect,
        path: &Path,
    ) {
        let corners = object.corners().map(|p| camera.screen(p, area));
        let bounds = Rect::from_points(&corners);
        if !bounds.intersects(area) {
            return;
        }
        let size = Vec2::new(object.size[0], object.size[1]) * camera.zoom;
        let center = camera.screen(object.center(), area);
        match &object.content {
            Content::Image { .. } | Content::ArchiveImage { .. } => {
                let source = object.content.image_source(path).unwrap();
                let (texture, error) = if size.max_elem() > 12.0 {
                    let c = object.options.crop;
                    self.assets.get(
                        &source,
                        size / Vec2::new(c[2] - c[0], c[3] - c[1]),
                        ui.ctx(),
                    )
                } else {
                    (None, None)
                };
                if let Some(texture) = texture {
                    let ts = texture.size();
                    self.media_sizes.insert(object.id, ts);
                    texture.paint_effects(
                        painter,
                        Rect::from_center_size(center, object.display_size(ts) * camera.zoom),
                        area,
                        object.angle,
                        object.effects(self.grayscale),
                    );
                } else {
                    painter.add(egui::Shape::convex_polygon(
                        corners.to_vec(),
                        Color32::from_rgb(39, 51, 65),
                        Stroke::new(1.0_f32, ui_theme::BORDER),
                    ));
                    if size.x > 80.0 && size.y > 35.0 {
                        painter.with_clip_rect(bounds.intersect(area)).text(
                            center,
                            Align2::CENTER_CENTER,
                            format!(
                                "{}\n{}",
                                if error.is_some() {
                                    "Missing / unavailable"
                                } else {
                                    "Loading…"
                                },
                                match &source {
                                    ImageSrc::File(path) => file_name(path),
                                    ImageSrc::Entry { name, .. } => name.clone(),
                                }
                            ),
                            FontId::proportional(12.0),
                            ui_theme::MUTED,
                        );
                    }
                }
            }
            Content::Video { source } => {
                let source = resolve(source, path);
                let (texture, error, playing) =
                    self.media.video_texture(object.id, &source, ui.ctx());
                if texture.is_none() {
                    painter.add(egui::Shape::convex_polygon(
                        corners.to_vec(),
                        Color32::from_rgb(13, 17, 22).gamma_multiply(object.options.opacity),
                        Stroke::new(
                            1.0_f32,
                            ui_theme::BORDER.gamma_multiply(object.options.opacity),
                        ),
                    ));
                }
                if let Some(texture) = texture {
                    let ts = texture.size();
                    self.media_sizes.insert(object.id, ts);
                    texture.paint_effects(
                        painter,
                        Rect::from_center_size(center, object.display_size(ts) * camera.zoom),
                        area,
                        object.angle,
                        object.effects(self.grayscale),
                    );
                }
                if size.x > 80.0 && size.y > 40.0 {
                    let label = if let Some(error) = error {
                        format!("Video unavailable: {error}")
                    } else {
                        format!(
                            "{} · {}",
                            if playing {
                                "Playing"
                            } else {
                                "Video · double-click to play"
                            },
                            file_name(&source)
                        )
                    };
                    let galley = painter.layout(
                        label,
                        FontId::proportional(12.0),
                        Color32::WHITE,
                        (size.x - 16.0).max(1.0),
                    );
                    painter.with_clip_rect(bounds.intersect(area)).add(
                        egui::epaint::TextShape::new(
                            corners[0] + rotate(Vec2::splat(8.0), object.angle),
                            galley,
                            Color32::WHITE,
                        )
                        .with_angle(object.angle),
                    );
                }
            }
            Content::Markdown { source } => {
                let source = resolve(source, path);
                if !self
                    .media
                    .paint_markdown(painter, object, camera, area, &source, ui.ctx())
                {
                    painter.add(egui::Shape::convex_polygon(
                        corners.to_vec(),
                        Color32::from_rgb(30, 37, 48),
                        Stroke::new(1.0_f32, ui_theme::BORDER),
                    ));
                    if size.x > 80.0 {
                        let text = self.media.error(object.id).unwrap_or("Loading Markdown…");
                        painter.with_clip_rect(bounds.intersect(area)).text(
                            center,
                            Align2::CENTER_CENTER,
                            text,
                            FontId::proportional(13.0),
                            ui_theme::MUTED,
                        );
                    }
                }
            }
            Content::Text { text, color } => {
                painter.add(egui::Shape::convex_polygon(
                    corners.to_vec(),
                    Color32::from_rgb(48, 43, 32),
                    Stroke::new(1.0_f32, Color32::from_rgb(91, 78, 53)),
                ));
                if size.x > 12.0 {
                    let font = FontId::proportional((18.0 * size.x / 280.0).clamp(2.0, 128.0));
                    let galley = painter.layout(
                        text.clone(),
                        font,
                        Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]),
                        (size.x - 16.0 * camera.zoom).max(10.0),
                    );
                    painter.with_clip_rect(bounds.intersect(area)).add(
                        egui::epaint::TextShape::new(
                            corners[0] + rotate(Vec2::splat(8.0 * camera.zoom), object.angle),
                            galley,
                            Color32::WHITE,
                        )
                        .with_angle(object.angle),
                    );
                }
            }
            Content::Ink {
                points,
                color,
                width,
            } => {
                let points: Vec<_> = points
                    .iter()
                    .map(|p| center + rotate(Vec2::new(p[0] * size.x, p[1] * size.y), object.angle))
                    .collect();
                let color = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        Stroke::new((width * camera.zoom).clamp(0.5, 100.0), color),
                    ));
                } else if let Some(p) = points.first() {
                    painter.circle_filled(*p, (width * camera.zoom * 0.5).max(0.5), color);
                }
            }
        }
        if self.selected.contains(&object.id) {
            painter.add(egui::Shape::closed_line(
                corners.to_vec(),
                Stroke::new(1.0_f32, Color32::LIGHT_BLUE),
            ));
        }
    }
    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        s: &mut Shared,
        area: Rect,
        fullscreen: &mut bool,
    ) {
        let camera = s.doc.camera;
        if response.drag_started() {
            let Some(start) = ui.input(|i| i.pointer.press_origin()) else {
                return;
            };
            let world = camera.world(start, area);
            let pan = self.tool == Tool::Hand
                || ui.input(|i| {
                    i.pointer.button_down(PointerButton::Middle) || i.key_down(Key::Space)
                });
            if ui.input(|i| i.pointer.button_down(PointerButton::Secondary)) {
                return;
            }
            if s.doc.canvas.locked && !pan {
                return;
            }
            let bounds = self.selection_bounds(&s.doc);
            let handles = bounds.map(|b| {
                (
                    camera.screen(b.right_bottom(), area),
                    camera.screen(b.center_top(), area) - Vec2::new(0.0, 24.0),
                )
            });
            let selected: Vec<_> = s
                .doc
                .objects
                .iter()
                .filter(|o| self.selected.contains(&o.id) && !o.options.locked)
                .cloned()
                .collect();
            let hit = self.hit(&s.doc, world, false);
            self.gesture = Some(if pan {
                Gesture::Pan { start, camera }
            } else if self.tool == Tool::Crop {
                let Some(before) = hit
                    .and_then(|id| {
                        s.doc
                            .objects
                            .iter()
                            .find(|o| o.id == id && o.visual_media())
                    })
                    .cloned()
                else {
                    return;
                };
                let Some(&texture) = self.media_sizes.get(&before.id) else {
                    return;
                };
                self.selected = HashSet::from([before.id]);
                Gesture::Crop {
                    start: world,
                    end: world,
                    before,
                    texture,
                }
            } else if self.tool == Tool::Draw {
                Gesture::Draw(vec![world])
            } else if self.tool == Tool::Select
                && handles.is_some_and(|(p, _)| p.distance(start) < 12.0)
            {
                s.checkpoint();
                Gesture::Scale {
                    start: world,
                    pivot: bounds.unwrap().center(),
                    objects: selected,
                }
            } else if self.tool == Tool::Select
                && handles.is_some_and(|(_, p)| p.distance(start) < 12.0)
            {
                s.checkpoint();
                Gesture::Rotate {
                    start: world,
                    pivot: bounds.unwrap().center(),
                    objects: selected,
                }
            } else if let Some(id) = hit {
                if !self.selected.contains(&id) {
                    if !ui.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                        self.selected.clear();
                    }
                    self.selected
                        .extend(s.doc.selection_for(id, ui.input(|i| i.modifiers.alt)));
                }
                s.checkpoint();
                Gesture::Move {
                    start: world,
                    objects: s
                        .doc
                        .objects
                        .iter()
                        .filter(|o| self.selected.contains(&o.id) && !o.options.locked)
                        .cloned()
                        .collect(),
                }
            } else {
                let base = if ui.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                    self.selected.clone()
                } else {
                    HashSet::new()
                };
                Gesture::Marquee { start: world, base }
            });
            s.gesture = Some(self.id);
        }
        if let (Some(gesture), Some(pointer)) = (&mut self.gesture, response.interact_pointer_pos())
        {
            let world = s.doc.camera.world(pointer, area);
            match gesture {
                Gesture::Pan { start, camera } => {
                    let delta = (pointer - *start) / camera.zoom;
                    s.doc.camera.center = [camera.center[0] - delta.x, camera.center[1] - delta.y];
                    s.changed();
                }
                Gesture::Marquee { start, base } => {
                    let r = Rect::from_two_pos(*start, world);
                    self.selected = base.clone();
                    self.selected.extend(
                        s.doc
                            .objects
                            .iter()
                            .filter(|o| {
                                o.bounds().intersects(r)
                                    && !o.options.locked
                                    && (!self.isolate || self.isolated_ids.contains(&o.id))
                            })
                            .map(|o| o.id),
                    );
                    if !ui.input(|i| i.modifiers.alt) {
                        let more: HashSet<_> = self
                            .selected
                            .iter()
                            .flat_map(|id| s.doc.selection_for(*id, false))
                            .collect();
                        self.selected.extend(more);
                    }
                }
                Gesture::Move { start, objects } => {
                    let mut delta = world - *start;
                    if ui.input(|i| i.modifiers.shift) {
                        if delta.x.abs() > delta.y.abs() {
                            delta.y = 0.0;
                        } else {
                            delta.x = 0.0;
                        }
                    }
                    if s.doc.canvas.snap && !ui.input(|i| i.modifiers.alt) {
                        if let Some(b) = objects.iter().map(Object::bounds).reduce(Rect::union) {
                            let target = b.min + delta;
                            let step = s.doc.canvas.step;
                            delta = Pos2::new(
                                (target.x / step).round() * step,
                                (target.y / step).round() * step,
                            ) - b.min;
                        }
                    }
                    apply_transform(&mut s.doc, objects, Pos2::ZERO, 1.0, 0.0, delta);
                    s.changed();
                }
                Gesture::Scale {
                    start,
                    pivot,
                    objects,
                } => {
                    let scale = (world.distance(*pivot) / start.distance(*pivot).max(1.0))
                        .clamp(0.05, 20.0);
                    apply_transform(&mut s.doc, objects, *pivot, scale, 0.0, Vec2::ZERO);
                    s.changed();
                }
                Gesture::Rotate {
                    start,
                    pivot,
                    objects,
                } => {
                    let a = *start - *pivot;
                    let b = world - *pivot;
                    let mut angle = b.y.atan2(b.x) - a.y.atan2(a.x);
                    if ui.input(|i| i.modifiers.shift) {
                        angle = (angle / 15_f32.to_radians()).round() * 15_f32.to_radians();
                    }
                    apply_transform(&mut s.doc, objects, *pivot, 1.0, angle, Vec2::ZERO);
                    s.changed();
                }
                Gesture::Draw(points) => {
                    if points.len() < 20_000
                        && points
                            .last()
                            .is_none_or(|p| p.distance(world) * camera.zoom > 1.5)
                    {
                        points.push(world);
                    }
                }
                Gesture::Crop { end, .. } => *end = world,
            }
        }
        if response.drag_stopped()
            || (self.gesture.is_some() && !ui.input(|i| i.pointer.any_down()))
        {
            if let Some(Gesture::Crop {
                start,
                end,
                before,
                texture,
            }) = &self.gesture
            {
                let rect = tools::crop_rect(*start, *end, before, *texture);
                let mut cropped = before.clone();
                if cropped.crop_to(rect, *texture) {
                    s.checkpoint();
                    if let Some(o) = s.doc.objects.iter_mut().find(|o| o.id == before.id) {
                        *o = cropped;
                    }
                    s.changed();
                }
                self.tool = Tool::Select;
            }
            if let Some(Gesture::Draw(points)) = self.gesture.take() {
                if !points.is_empty() && s.doc.objects.len() < MAX_OBJECTS {
                    let total: usize = s
                        .doc
                        .objects
                        .iter()
                        .map(|o| {
                            if let Content::Ink { points, .. } = &o.content {
                                points.len()
                            } else {
                                0
                            }
                        })
                        .sum();
                    if total + points.len() <= MAX_POINTS {
                        s.checkpoint();
                        let bounds = Rect::from_points(&points);
                        let center = bounds.center();
                        let size = bounds.size().max(Vec2::splat(1.0));
                        let id = s.doc.next_id();
                        s.doc.objects.push(Object {
                            id,
                            center: [center.x, center.y],
                            size: [size.x, size.y],
                            angle: 0.0,
                            options: Default::default(),
                            content: Content::Ink {
                                points: points
                                    .into_iter()
                                    .map(|p| [(p.x - center.x) / size.x, (p.y - center.y) / size.y])
                                    .collect(),
                                color: self.color,
                                width: (self.pen_width / camera.zoom).clamp(0.1, 100.0),
                            },
                        });
                        self.selected = HashSet::from([id]);
                        s.changed();
                    }
                }
            }
            self.gesture = None;
            s.gesture = None;
        }
        if response.clicked_by(PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let world = camera.world(p, area);
                if self.tool == Tool::Text {
                    self.note(s, world);
                } else if self.tool == Tool::Select {
                    let hit = self.hit(&s.doc, world, false);
                    let additive = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                    if !additive {
                        self.selected.clear();
                    }
                    if let Some(id) = hit {
                        if additive && self.selected.contains(&id) {
                            let ids = s.doc.selection_for(id, ui.input(|i| i.modifiers.alt));
                            self.selected.retain(|id| !ids.contains(id));
                        } else {
                            self.selected
                                .extend(s.doc.selection_for(id, ui.input(|i| i.modifiers.alt)));
                        }
                    }
                }
            }
        }
        if response.double_clicked() && self.tool == Tool::Select {
            if let Some(object) = s
                .doc
                .objects
                .iter()
                .rev()
                .find(|o| self.selected.contains(&o.id))
            {
                match &object.content {
                    Content::Text { text, .. }
                        if !object.options.locked && !s.doc.canvas.locked =>
                    {
                        self.edit_text = Some((object.id, text.clone()))
                    }
                    Content::Video { .. } => self.media.toggle(object.id, ui.ctx()),
                    Content::Markdown { .. } => self.media.open_reader(object.id),
                    _ => *fullscreen = true,
                }
            } else {
                *fullscreen = true;
            }
        }
    }
    fn text_window(&mut self, ctx: &egui::Context) {
        let Some((id, text)) = &mut self.edit_text else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Text note")
            .id(egui::Id::new(("board-note", self.id)))
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .desired_rows(7)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Apply").clicked() {
                    apply = true;
                }
                ui.small("Closing this editor cancels the draft.");
            });
        if apply {
            let mut s = self.shared.lock().unwrap();
            if s.doc.canvas.locked
                || s.doc
                    .objects
                    .iter()
                    .any(|o| o.id == *id && o.options.locked)
            {
                self.edit_text = None;
                return;
            }
            if s.gesture.is_none() {
                s.checkpoint();
                if let Some(o) = s.doc.objects.iter_mut().find(|o| o.id == *id) {
                    if let Content::Text { text: old, .. } = &mut o.content {
                        if o.options.locked {
                            self.edit_text = None;
                            return;
                        }
                        *old = text.chars().take(100_000).collect();
                        s.changed();
                    }
                }
                self.edit_text = None;
            }
        } else if !open {
            self.edit_text = None;
        }
    }
}

fn apply_transform(
    doc: &mut Document,
    initial: &[Object],
    pivot: Pos2,
    scale: f32,
    angle: f32,
    translation: Vec2,
) {
    for before in initial {
        if before.options.locked || doc.canvas.locked {
            continue;
        }
        if let Some(after) = doc.objects.iter_mut().find(|o| o.id == before.id) {
            let center = pivot + rotate((before.center() - pivot) * scale, angle) + translation;
            after.center = [center.x.clamp(-1e9, 1e9), center.y.clamp(-1e9, 1e9)];
            after.size = before.size.map(|v| (v * scale).clamp(0.1, 1e7));
            after.angle = (before.angle + angle).rem_euclid(std::f32::consts::TAU);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_selected_images_from_either_pane_and_archive_survives_reopen() {
        use std::io::Write;
        let root = super::super::tests::fixture("cross-pane-archive");
        let images = root.join("images");
        std::fs::create_dir(&images).unwrap();
        for (name, color) in [("01.png", [240, 40, 20]), ("02.png", [20, 210, 50])] {
            image::RgbImage::from_pixel(40, 60, image::Rgb(color))
                .save(images.join(name))
                .unwrap();
        }
        let archive = root.join("Референсы.cbz");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for name in ["01.png", "02.png"] {
            zip.start_file(
                format!("страницы/{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&std::fs::read(images.join(name)).unwrap())
                .unwrap();
        }
        zip.finish().unwrap();
        let original = std::fs::read(&archive).unwrap();
        for source_pane in [0, 1] {
            for archived in [false, true] {
                for details in [false, true] {
                    let path = root.join(format!("{source_pane}-{archived}-{details}.vcboard"));
                    write(&path, &Document::new("Drag test".into())).unwrap();
                    let ctx = egui::Context::default();
                    let mut app = App::new(None);
                    app.set_split_preview(true, &ctx);
                    app.switch_pane(1 - source_pane);
                    app.open_path(&path, &ctx);
                    app.switch_pane(source_pane);
                    app.select(
                        if archived {
                            Node::ArchiveDir {
                                archive: archive.clone(),
                                prefix: "страницы/".into(),
                            }
                        } else {
                            Node::Dir(images.clone())
                        },
                        &ctx,
                    );
                    app.files.details = details;
                    app.files.selected = HashSet::from([0, 1]);
                    assert_eq!(app.selected_board_sources().len(), 2);
                    let run = |app: &mut App, events: Vec<egui::Event>| {
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    Pos2::ZERO,
                                    Vec2::new(1400.0, 900.0),
                                )),
                                events,
                                ..Default::default()
                            },
                            |ctx| {
                                // Match the real application's ordering: previews before the browser.
                                egui::SidePanel::right("preview")
                                    .exact_width(800.0)
                                    .show(ctx, |ui| app.show_preview_panel(ui));
                                egui::CentralPanel::default().show(ctx, |ui| app.file_gallery(ui));
                            },
                        );
                    };
                    run(&mut app, vec![]);
                    run(&mut app, vec![]);
                    let start = app.files.item_rects[0].center();
                    let target = app
                        .inactive
                        .as_ref()
                        .unwrap()
                        .board
                        .as_ref()
                        .unwrap()
                        .area
                        .center();
                    run(
                        &mut app,
                        vec![egui::Event::PointerMoved(start), button(start, true)],
                    );
                    run(
                        &mut app,
                        vec![egui::Event::PointerMoved(start + Vec2::splat(15.0))],
                    );
                    if archived {
                        assert_eq!(
                            egui::DragAndDrop::payload::<ImageDrag>(&ctx)
                                .expect("archive row starts a drag")
                                .sources
                                .len(),
                            2
                        );
                    } else {
                        assert_eq!(
                            egui::DragAndDrop::payload::<crate::clipboard::Files>(&ctx)
                                .expect("file row starts a drag")
                                .paths
                                .len(),
                            2
                        );
                    }
                    run(&mut app, vec![egui::Event::PointerMoved(target)]);
                    run(&mut app, vec![button(target, false)]);
                    let board = app.inactive.as_mut().unwrap().board.as_mut().unwrap();
                    assert_eq!(
                        board.shared.lock().unwrap().doc.objects.len(),
                        2,
                        "source pane {source_pane}, archive={archived}, details={details}"
                    );
                    assert!(
                        app.files.pending.is_none(),
                        "Dropping on a board must not schedule a file move"
                    );
                    board.flush().unwrap();
                    drop(app);
                    let doc = read(&path).unwrap();
                    assert_eq!(doc.objects.len(), 2);
                    for (index, object) in doc.objects.iter().enumerate() {
                        let source = object.content.image_source(&path).unwrap();
                        if archived {
                            assert!(
                                matches!(&source,ImageSrc::Entry{archive:a,name} if a==&archive && name==&format!("страницы/0{}.png",index+1))
                            );
                        }
                        assert_eq!(decode_thumbnail(&source, false).unwrap().size, [40, 60]);
                    }
                }
            }
        }
        assert_eq!(
            std::fs::read(&archive).unwrap(),
            original,
            "Archive remains unchanged"
        );
        assert!(images.join("01.png").exists() && images.join("02.png").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn frame(editor: &mut Editor, ctx: &egui::Context, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    editor.show(ui);
                });
            },
        );
    }
    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn drag(editor: &mut Editor, ctx: &egui::Context, start: Pos2, end: Pos2) {
        frame(
            editor,
            ctx,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(
            editor,
            ctx,
            vec![egui::Event::PointerMoved(start + Vec2::new(8.0, 8.0))],
        );
        frame(editor, ctx, vec![egui::Event::PointerMoved(end)]);
        frame(editor, ctx, vec![button(end, false)]);
    }
    #[test]
    fn media_controls_do_not_interrupt_drag_and_markdown_opens_reader() {
        let root = super::super::tests::fixture("media-gestures");
        let path = root.join("board.vcboard");
        let markdown = root.join("notes.md");
        std::fs::write(&markdown, "# Notes\n\n**Formatted** document").unwrap();
        let mut doc = Document::new("Media".into());
        doc.version = 2;
        for (id, center, content) in [
            (
                1,
                [-220.0, 0.0],
                Content::Video {
                    source: root.join("missing.mp4"),
                },
            ),
            (2, [220.0, 0.0], Content::Markdown { source: markdown }),
        ] {
            doc.objects.push(Object {
                id,
                center,
                size: [180.0, 140.0],
                angle: 0.0,
                options: Default::default(),
                content,
            });
        }
        write(&path, &doc).unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::open(&path).unwrap();
        frame(&mut editor, &ctx, vec![]);
        let center = editor.area.center() - Vec2::new(220.0, 0.0);
        drag(&mut editor, &ctx, center, center + Vec2::new(60.0, 35.0));
        let moved = editor.shared.lock().unwrap().doc.objects[0].center;
        assert_eq!(moved, [-160.0, 35.0]);
        frame(&mut editor, &ctx, vec![]);
        assert_eq!(editor.controls_id, Some(1));
        let screen = {
            let s = editor.shared.lock().unwrap();
            s.doc
                .camera
                .screen(Pos2::from(s.doc.objects[0].center), editor.area)
        };
        assert!(screen.distance(center + Vec2::new(60.0, 35.0)) < 0.1);
        let center = {
            let s = editor.shared.lock().unwrap();
            s.doc
                .camera
                .screen(Pos2::from(s.doc.objects[1].center), editor.area)
        };
        drag(&mut editor, &ctx, center, center + Vec2::new(-50.0, 20.0));
        let moved = editor.shared.lock().unwrap().doc.objects[1].center;
        assert_eq!(moved, [170.0, 20.0]);
        frame(&mut editor, &ctx, vec![]);
        assert_eq!(editor.controls_id, Some(2));
        let center = {
            let s = editor.shared.lock().unwrap();
            s.doc
                .camera
                .screen(Pos2::from(s.doc.objects[1].center), editor.area)
        };
        for _ in 0..2 {
            frame(
                &mut editor,
                &ctx,
                vec![egui::Event::PointerMoved(center), button(center, true)],
            );
            frame(&mut editor, &ctx, vec![button(center, false)]);
        }
        assert!(editor.media.reader_open());
        editor.flush().unwrap();
        drop(editor);
        assert_eq!(read(&path).unwrap().objects[1].center, [170.0, 20.0]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn canvas_move_resize_rotate_draw_undo_and_shared_document() {
        let root = super::super::tests::fixture("gestures");
        let path = root.join("board.vcboard");
        let mut doc = Document::new("Test".into());
        doc.objects.push(Object {
            id: 1,
            center: [0.0, 0.0],
            size: [180.0, 100.0],
            angle: 0.0,
            options: Default::default(),
            content: Content::Text {
                text: "Reference".into(),
                color: [255; 4],
            },
        });
        write(&path, &doc).unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::open(&path).unwrap();
        let other = Editor::open(&path).unwrap();
        frame(&mut editor, &ctx, vec![]);
        let center = editor.area.center();
        drag(&mut editor, &ctx, center, center + Vec2::new(60.0, 35.0));
        {
            let s = editor.shared.lock().unwrap();
            assert_eq!(s.doc.objects[0].center, [60.0, 35.0]);
            assert_eq!(s.undo.len(), 1);
        }
        {
            let mut s = editor.shared.lock().unwrap();
            s.undo(false);
            assert_eq!(s.doc.objects[0].center, [0.0, 0.0]);
            s.undo(true);
        }
        let (handle, pivot) = {
            let s = editor.shared.lock().unwrap();
            let b = editor.selection_bounds(&s.doc).unwrap();
            (
                s.doc.camera.screen(b.max, editor.area),
                s.doc.camera.screen(b.center(), editor.area),
            )
        };
        drag(&mut editor, &ctx, handle, pivot + (handle - pivot) * 1.5);
        assert!((editor.shared.lock().unwrap().doc.objects[0].size[0] - 270.0).abs() < 1.0);
        let (handle, pivot) = {
            let s = editor.shared.lock().unwrap();
            let b = editor.selection_bounds(&s.doc).unwrap();
            (
                s.doc.camera.screen(b.center_top(), editor.area) - Vec2::new(0.0, 24.0),
                s.doc.camera.screen(b.center(), editor.area),
            )
        };
        drag(
            &mut editor,
            &ctx,
            handle,
            pivot + rotate(handle - pivot, 0.7),
        );
        assert!((editor.shared.lock().unwrap().doc.objects[0].angle - 0.7).abs() < 0.03);
        editor.tool = Tool::Draw;
        let p = editor.area.center() + Vec2::new(-250.0, 80.0);
        drag(&mut editor, &ctx, p, p + Vec2::new(90.0, 60.0));
        {
            let s = editor.shared.lock().unwrap();
            assert_eq!(s.doc.objects.len(), 2);
            assert!(matches!(s.doc.objects[1].content, Content::Ink { .. }));
            s.doc.validate().unwrap();
        }
        assert_eq!(other.shared.lock().unwrap().doc.objects.len(), 2);
        editor.flush().unwrap();
        assert_eq!(read(&path).unwrap().objects.len(), 2);
        drop(editor);
        drop(other);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn drop_adds_references_and_delete_only_removes_board_objects() {
        let root = super::super::tests::fixture("drop");
        let path = root.join("board.vcboard");
        let image = root.join("image.png");
        image::RgbImage::new(8, 8).save(&image).unwrap();
        write(&path, &Document::new("Drop".into())).unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::open(&path).unwrap();
        frame(&mut editor, &ctx, vec![]);
        let p = editor.area.center();
        egui::DragAndDrop::set_payload(
            &ctx,
            crate::clipboard::Files {
                paths: vec![image.clone()],
                cut: true,
            },
        );
        frame(
            &mut editor,
            &ctx,
            vec![egui::Event::PointerMoved(p), button(p, true)],
        );
        frame(&mut editor, &ctx, vec![button(p, false)]);
        assert_eq!(editor.shared.lock().unwrap().doc.objects.len(), 1);
        let shared = editor.shared.clone();
        {
            let mut s = shared.lock().unwrap();
            editor.delete_selected(&mut s);
            assert!(s.doc.objects.is_empty());
            s.undo(false);
            assert_eq!(s.doc.objects.len(), 1);
        }
        assert!(image.exists());
        drop(editor);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn video_and_markdown_copy_preserves_types_layout_and_originals_on_reopen() {
        let root = super::super::tests::fixture("media-owned");
        let original = root.join("original");
        let owned = root.join("owned");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&owned).unwrap();
        let video = original.join("motion.mp4");
        let md = original.join("notes.md");
        // This tests reference/copy semantics; playback uses actual generated clips in media tests.
        std::fs::write(&video, b"unchanged video bytes").unwrap();
        std::fs::write(&md, "# Notes\n**Bold**").unwrap();
        let path = owned.join("board.vcboard");
        write(&path, &Document::new("Media".into())).unwrap();
        let mut editor = Editor::open(&path).unwrap();
        editor.set_collection(true);
        editor.add_files(&[video.clone(), md.clone()], None);
        let ctx = egui::Context::default();
        {
            let mut s = editor.shared.lock().unwrap();
            s.doc.objects[0].angle = 0.7;
            s.doc.objects[1].size = [300.0, 400.0];
            s.changed();
        }
        editor.flush().unwrap();
        let before = read(&path).unwrap();
        assert_eq!(before.version, 2);
        assert!(matches!(before.objects[0].content, Content::Video { .. }));
        assert!(matches!(
            before.objects[1].content,
            Content::Markdown { .. }
        ));
        editor.copy_selection_into_folder(&ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while editor.importing() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        editor.flush().unwrap();
        drop(editor);
        let restored = read(&path).unwrap();
        assert_eq!(restored.objects[0].angle, 0.7);
        assert_eq!(restored.objects[1].size, [300.0, 400.0]);
        let Content::Video { source } = &restored.objects[0].content else {
            panic!()
        };
        assert!(source.is_relative());
        assert_eq!(
            std::fs::read(resolve(source, &path)).unwrap(),
            std::fs::read(&video).unwrap()
        );
        let Content::Markdown { source } = &restored.objects[1].content else {
            panic!()
        };
        assert!(source.is_relative());
        assert_eq!(
            std::fs::read(resolve(source, &path)).unwrap(),
            std::fs::read(&md).unwrap()
        );
        let mut rebased = restored.clone();
        let relocated = root.join("export/board.vcboard");
        rebased.rebase(&path, &relocated);
        for (old, new) in restored.objects.iter().zip(&rebased.objects) {
            let (Content::Video { source: a } | Content::Markdown { source: a }) = &old.content
            else {
                panic!()
            };
            let (Content::Video { source: b } | Content::Markdown { source: b }) = &new.content
            else {
                panic!()
            };
            assert_eq!(
                std::fs::canonicalize(resolve(a, &path)).unwrap(),
                std::fs::canonicalize(resolve(b, &relocated)).unwrap()
            );
        }
        assert!(video.is_file() && md.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}
