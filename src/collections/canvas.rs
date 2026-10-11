use super::*;

#[derive(Default)]
pub struct State {
    relocation: Option<(
        mpsc::Receiver<std::result::Result<(), String>>,
        Option<String>,
    )>,
    pub id: Option<String>,
    pub editor: Option<board::Editor>,
    pub fullscreen: bool,
    #[cfg(test)]
    pub blank: Option<Rect>,
}
impl App {
    pub(crate) fn clear_collection_canvas(&mut self) -> bool {
        if let Some(editor) = &self.canvas.editor {
            if let Err(error) = editor.flush() {
                self.file_message = error.to_string();
                return false;
            }
        }
        self.canvas.editor = None;
        self.canvas.id = None;
        true
    }
    pub(crate) fn show_collection_preview(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Preview");
            ui.weak("File / Collection");
        });
        ui.separator();
        let rect = ui.available_rect_before_wrap();
        let y = rect.top() + rect.height() * self.preview_fraction;
        let divider = Rect::from_min_max(
            Pos2::new(rect.left(), y - 3.0),
            Pos2::new(rect.right(), y + 3.0),
        );
        let drag = ui.interact(divider, ui.id().with("collection_divider"), Sense::drag());
        if drag.dragged() {
            self.preview_fraction = (self.preview_fraction
                + ui.input(|i| i.pointer.delta().y) / rect.height().max(1.0))
            .clamp(0.2, 0.8);
        }
        drag.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        ui.painter().rect_filled(divider, 0.0, ui_theme::BORDER);
        let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), y - 3.0));
        let bottom = Rect::from_min_max(Pos2::new(rect.left(), y + 3.0), rect.max);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(top), |ui| {
            ui.set_clip_rect(top.intersect(ui.clip_rect()));
            ui.push_id(
                ("collection_file_preview", self.active_pane, self.active_tab),
                |ui| self.show_preview_content(ui, self.active_pane),
            );
        });
        let fullscreen = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(bottom), |ui| {
                ui.set_clip_rect(bottom.intersect(ui.clip_rect()));
                ui.push_id("collection_canvas", |ui| {
                    self.canvas
                        .editor
                        .as_mut()
                        .is_some_and(|editor| editor.show(ui))
                })
                .inner
            })
            .inner;
        if fullscreen {
            self.canvas.fullscreen = true;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
    }
    pub(crate) fn show_collection_fullscreen(&mut self, ctx: &egui::Context) {
        let mut exit = ctx.input(|i| i.key_pressed(Key::Escape));
        egui::TopBottomPanel::bottom("collection_fullscreen_bar").show(ctx, |ui| {
            exit |= ui.button("Exit fullscreen · Esc").clicked();
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.push_id("collection_canvas", |ui| {
                if let Some(editor) = &mut self.canvas.editor {
                    exit |= editor.show(ui);
                }
            });
        });
        if exit {
            self.canvas.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }
    pub(crate) fn collection_settings(&mut self, ctx: &egui::Context) {
        if let Some((rx, _)) = &self.canvas.relocation {
            match rx.try_recv() {
                Ok(result) => {
                    let (_, selected) = self.canvas.relocation.take().unwrap();
                    match result {
                        Ok(()) => {
                            self.settings.collections_root =
                                storage::root().to_string_lossy().into();
                            self.settings_message = match self.settings.save() {
                                Ok(()) => "Collection folder changed. The previous copy was kept.".into(),
                                Err(error) => format!("Library copied, but its setting could not be saved: {error}. Choose the new folder again before restarting."),
                            };
                        }
                        Err(e) => self.settings_message = e,
                    }
                    if let Some(id) = selected {
                        self.open_collection_board(&id, ctx);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.canvas.relocation = None;
                    self.settings_message =
                        "Library copy was interrupted; the original was kept.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let request =
            ctx.data_mut(|d| d.remove_temp::<PathBuf>(egui::Id::new("collection_root_request")));
        let Some(target) = request else {
            return;
        };
        if board::imports_running() || storage::busy() {
            self.settings_message =
                "Wait for image imports to finish before moving the library.".into();
            return;
        }
        let selected = self.canvas.id.clone();
        if !self.clear_collection_canvas() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.canvas.relocation = Some((rx, selected));
        self.settings_message = "Copying collection library…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = storage::relocate(&target).map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
