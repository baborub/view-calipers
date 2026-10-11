use super::*;
use std::collections::BTreeMap;
#[cfg(test)]
mod tests;

pub(super) fn crop_rect(start: Pos2, end: Pos2, object: &Object, texture: [usize; 2]) -> Rect {
    let size = object.display_size(texture);
    let point = |p: Pos2| {
        let local = rotate(p - object.center(), -object.angle) / size;
        Pos2::new(local.x + 0.5, local.y + 0.5)
    };
    Rect::from_two_pos(point(start), point(end))
        .intersect(Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)))
}
impl Editor {
    pub(super) fn command(&mut self, s: &mut Shared, command: Command) {
        if s.gesture.is_some() {
            return;
        }
        let mut doc = s.doc.clone();
        let mut selected = self.selected.clone();
        if doc.apply(
            &mut selected,
            command,
            self.area.width() / self.area.height().max(1.0),
        ) && doc != s.doc
        {
            if let Err(e) = doc.validate() {
                self.message = e.to_string();
                return;
            }
            s.checkpoint();
            s.doc = doc;
            self.selected = selected;
            s.changed();
        }
    }
    pub(super) fn reference_actions(&mut self, ui: &mut egui::Ui, s: &mut Shared) {
        ui.add_enabled_ui(!self.selected.is_empty() && !s.doc.canvas.locked, |ui| {
            for (label, command) in [
                ("Duplicate · Ctrl+D", Command::Duplicate),
                ("Group · Ctrl+G", Command::Group),
                ("Ungroup · Ctrl+Shift+G", Command::Ungroup),
                ("Lock / unlock · Alt+L", Command::ToggleLock),
            ] {
                if ui.button(label).clicked() {
                    self.command(s, command);
                    ui.close_menu();
                }
            }
            ui.menu_button("Image / video", |ui| {
                for (label, command) in [
                    ("Flip horizontally · Alt+Shift+H", Command::FlipX),
                    ("Flip vertically · Alt+Shift+V", Command::FlipY),
                    ("Grayscale · Alt+G", Command::Grayscale),
                    ("Reset crop", Command::ResetCrop),
                ] {
                    if ui.button(label).clicked() {
                        self.command(s, command);
                        ui.close_menu();
                    }
                }
                if ui.button("Crop · C, then drag on the image").clicked() {
                    self.tool = Tool::Crop;
                    ui.close_menu();
                }
                let mut opacity = s
                    .doc
                    .objects
                    .iter()
                    .find(|o| self.selected.contains(&o.id) && o.visual_media())
                    .map(|o| o.options.opacity)
                    .unwrap_or(1.0);
                let slider = ui.add(
                    egui::Slider::new(&mut opacity, 0.0..=1.0)
                        .text("Opacity")
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                // A single undo snapshot for the whole slider drag; keyboard/click changes are atomic.
                if slider.changed() {
                    if slider.drag_started()
                        || !slider.dragged()
                        || !ui.ctx().data(|d| {
                            d.get_temp::<bool>(ui.id().with("opacity_edit"))
                                .unwrap_or(false)
                        })
                    {
                        s.checkpoint();
                    }
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(ui.id().with("opacity_edit"), slider.dragged())
                    });
                    s.doc
                        .apply(&mut self.selected, Command::Opacity(opacity), 1.0);
                    s.changed();
                }
                if slider.drag_stopped() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(ui.id().with("opacity_edit"), false));
                }
            });
        });
        if ui.button("Objects & groups…").clicked() {
            self.items_open = true;
            ui.close_menu();
        }
    }
    pub(super) fn arrange_menu(&mut self, ui: &mut egui::Ui, s: &mut Shared) {
        ui.small("Selected items, or all unlocked items if none are selected");
        ui.add_enabled_ui(!s.doc.canvas.locked, |ui| {
            for (label, command) in [
                ("Pack · Ctrl+P", Command::Pack),
                ("Grid layout", Command::Grid),
                ("Horizontal row", Command::Row),
                ("Vertical column", Command::Column),
            ] {
                if ui.button(label).clicked() {
                    self.command(s, command);
                    ui.close_menu();
                }
            }
            ui.menu_button("Align", |ui| {
                for (index, label) in [
                    "Left",
                    "Center horizontally",
                    "Right",
                    "Top",
                    "Center vertically",
                    "Bottom",
                ]
                .iter()
                .enumerate()
                {
                    if ui.button(*label).clicked() {
                        self.command(s, Command::Align(index as u8));
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Same size", |ui| {
                for (index, label) in ["Width", "Height", "Area"].iter().enumerate() {
                    if ui.button(*label).clicked() {
                        self.command(s, Command::Normalize(index as u8));
                        ui.close_menu();
                    }
                }
            });
            let mut gap = s.doc.canvas.gap;
            if ui
                .add(
                    egui::DragValue::new(&mut gap)
                        .range(0.0..=1000.0)
                        .prefix("Gap: ")
                        .suffix(" px"),
                )
                .changed()
            {
                s.checkpoint();
                s.doc.canvas.gap = gap;
                s.changed();
            }
        });
    }
    pub(super) fn reference_view(&mut self, ui: &mut egui::Ui, s: &mut Shared) {
        if ui
            .selectable_label(self.focus_camera.is_some(), "Focus / return · Z")
            .clicked()
        {
            self.toggle_focus(s);
            ui.close_menu();
        }
        if ui
            .checkbox(&mut self.isolate, "Isolate selection")
            .on_hover_text("Show only selected objects; turn off to restore the full canvas")
            .changed()
        {
            self.isolated_ids = self.selected.clone();
            if self.isolated_ids.is_empty() {
                self.isolate = false;
            }
        }
        if ui.button("Fit whole canvas").clicked() {
            self.isolate = false;
            self.selected.clear();
            self.need_fit = true;
            ui.close_menu();
        }
        if ui.button("Objects & groups…").clicked() {
            self.items_open = true;
            ui.close_menu();
        }
        ui.separator();
        let mut canvas = s.doc.canvas.clone();
        ui.checkbox(&mut canvas.locked, "Lock canvas");
        ui.checkbox(&mut canvas.grayscale, "Grayscale images & videos");
        ui.checkbox(&mut canvas.grid, "Show canvas grid");
        ui.checkbox(&mut canvas.snap, "Snap to grid · Alt temporarily disables");
        if canvas.snap {
            ui.add(
                egui::DragValue::new(&mut canvas.step)
                    .range(1.0..=1000.0)
                    .prefix("Grid step: ")
                    .suffix(" px"),
            );
        }
        if canvas != s.doc.canvas {
            s.checkpoint();
            s.doc.canvas = canvas;
            s.changed();
        }
        if ui.button("Unlock all objects").clicked() {
            self.command(s, Command::UnlockAll);
            ui.close_menu();
        }
        ui.separator();
        let key = egui::Id::new("board_always_on_top");
        let mut top = ui.ctx().data(|d| d.get_temp::<bool>(key).unwrap_or(false));
        if ui.checkbox(&mut top, "Window always on top").changed() {
            ui.ctx().data_mut(|d| d.insert_temp(key, top));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                }));
        }
        ui.small("Alt+click: select inside a group · Shift+drag: constrain axis");
        ui.small("Z: focus/return · Left/Right: focus previous/next");
    }
    fn toggle_focus(&mut self, s: &mut Shared) {
        if let Some(camera) = self.focus_camera.take() {
            s.doc.camera = camera;
        } else {
            self.focus_camera = Some(s.doc.camera);
            self.fit_document(&mut s.doc, self.area);
        }
        s.changed();
    }
    fn focus_step(&mut self, s: &mut Shared, delta: isize) {
        if s.doc.objects.is_empty() {
            return;
        }
        let index = s
            .doc
            .objects
            .iter()
            .position(|o| self.selected.contains(&o.id))
            .unwrap_or(0);
        let index = (index as isize + delta).rem_euclid(s.doc.objects.len() as isize) as usize;
        self.focus_camera.get_or_insert(s.doc.camera);
        self.selected = HashSet::from([s.doc.objects[index].id]);
        if self.isolate {
            self.isolated_ids = self.selected.clone();
        }
        self.fit_document(&mut s.doc, self.area);
        s.changed();
    }
    pub(super) fn reference_shortcuts(&mut self, ui: &egui::Ui, s: &mut Shared) {
        use egui::Modifiers as M;
        let commands = [
            (M::CTRL, Key::D, Command::Duplicate),
            (M::CTRL | M::SHIFT, Key::G, Command::Ungroup),
            (M::CTRL, Key::G, Command::Group),
            (M::ALT, Key::L, Command::ToggleLock),
            (M::ALT, Key::G, Command::Grayscale),
            (M::ALT | M::SHIFT, Key::H, Command::FlipX),
            (M::ALT | M::SHIFT, Key::V, Command::FlipY),
            (M::CTRL, Key::P, Command::Pack),
            (M::CTRL, Key::ArrowLeft, Command::Align(0)),
            (M::CTRL, Key::ArrowRight, Command::Align(2)),
            (M::CTRL, Key::ArrowUp, Command::Align(3)),
            (M::CTRL, Key::ArrowDown, Command::Align(5)),
        ];
        for (modifiers, key, command) in commands {
            if ui.input_mut(|i| i.consume_key(modifiers, key)) {
                self.command(s, command);
            }
        }
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::Z)) {
            self.toggle_focus(s);
        }
        for (key, delta) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
            if ui.input_mut(|i| i.consume_key(M::NONE, key)) {
                self.focus_step(s, delta);
            }
        }
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::Escape)) {
            if matches!(self.gesture, Some(Gesture::Crop { .. })) {
                self.gesture = None;
                s.gesture = None;
            }
            self.tool = Tool::Select;
            if self.focus_camera.is_some() {
                self.toggle_focus(s);
            }
        }
    }
    pub(super) fn hit(&self, doc: &Document, point: Pos2, include_locked: bool) -> Option<u64> {
        let visible = |o: &&Object| {
            (include_locked || !o.options.locked)
                && (!self.isolate || self.isolated_ids.contains(&o.id))
        };
        doc.objects
            .iter()
            .rev()
            .filter(visible)
            .find(|o| o.hit(point, 3.0 / doc.camera.zoom))
            .map(|o| o.id)
            .or_else(|| {
                self.group_regions(doc)
                    .iter()
                    .rev()
                    .find_map(|(group, bounds)| {
                        if bounds.contains(point) {
                            doc.objects
                                .iter()
                                .filter(visible)
                                .find(|o| o.options.group == Some(*group))
                                .map(|o| o.id)
                        } else {
                            None
                        }
                    })
            })
    }
    fn group_regions(&self, doc: &Document) -> BTreeMap<u64, Rect> {
        let mut bounds = BTreeMap::<u64, Rect>::new();
        for o in &doc.objects {
            if self.isolate && !self.isolated_ids.contains(&o.id) {
                continue;
            }
            if let Some(group) = o.options.group {
                bounds
                    .entry(group)
                    .and_modify(|r| *r = r.union(o.bounds()))
                    .or_insert_with(|| o.bounds());
            }
        }
        bounds
            .into_iter()
            .map(|(id, r)| (id, operations::group_rect(r, doc.camera.zoom)))
            .collect()
    }
    pub(super) fn paint_groups(&self, painter: &egui::Painter, doc: &Document, area: Rect) {
        for (id, bounds) in self.group_regions(doc) {
            let Some(name) = doc.groups.get(&id) else {
                continue;
            };
            let r = Rect::from_min_max(
                doc.camera.screen(bounds.min, area),
                doc.camera.screen(bounds.max, area),
            );
            painter.rect(
                r,
                4.0,
                Color32::from_rgba_unmultiplied(65, 106, 140, 28),
                Stroke::new(1.0_f32, Color32::from_rgb(60, 94, 120)),
            );
            painter.with_clip_rect(r.intersect(area)).text(
                r.left_top() + Vec2::splat(4.0),
                Align2::LEFT_TOP,
                name,
                FontId::proportional(12.0),
                Color32::from_rgb(147, 189, 217),
            );
        }
    }
    pub(super) fn paint_crop(&self, painter: &egui::Painter, camera: Camera, area: Rect) {
        if let Some(Gesture::Crop {
            start,
            end,
            before,
            texture,
        }) = &self.gesture
        {
            let r = crop_rect(*start, *end, before, *texture);
            let size = before.display_size(*texture);
            let points = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ]
            .map(|p| {
                camera.screen(
                    before.center() + rotate((p - Pos2::new(0.5, 0.5)) * size, before.angle),
                    area,
                )
            });
            painter.add(egui::Shape::closed_line(
                points.to_vec(),
                Stroke::new(2.0_f32, Color32::from_rgb(255, 204, 102)),
            ));
        }
    }
    pub(super) fn items_window(&mut self, ctx: &egui::Context) {
        if !self.items_open {
            return;
        }
        let mut open = true;
        let shared = self.shared.clone();
        let mut s = shared.lock().unwrap();
        egui::Window::new("Canvas objects & groups").id(egui::Id::new(("board-objects",self.id)))
            .open(&mut open).default_size(Vec2::new(360.0,420.0)).show(ctx,|ui| {
                ui.small("Select locked objects here to unlock them. Alt+click on canvas selects inside a group.");
                if ui.button("Unlock all").clicked() {self.command(&mut s,Command::UnlockAll);}
                egui::ScrollArea::vertical().max_height(500.0).show(ui,|ui| {
                    let groups:Vec<_>=s.doc.groups.iter().map(|(id,name)|(*id,name.clone())).collect();
                    for (id,name) in groups {
                        egui::CollapsingHeader::new(&name).id_salt(id).default_open(true).show(ui,|ui| {
                            ui.horizontal(|ui| {
                                if ui.small_button("Select group").clicked() {
                                    self.selected=s.doc.objects.iter().filter(|o|o.options.group==Some(id)).map(|o|o.id).collect();self.focused=true;
                                }
                                let mut next=name.clone();
                                if ui.add(egui::TextEdit::singleline(&mut next).desired_width(150.0).char_limit(200)).changed() {
                                    s.checkpoint();s.doc.groups.insert(id,next);s.changed();
                                }
                            });
                            let ids:Vec<_>=s.doc.objects.iter().filter(|o|o.options.group==Some(id)).map(|o|o.id).collect();
                            for id in ids {self.object_row(ui,&mut s,id);}
                        });
                    }
                    let ids:Vec<_>=s.doc.objects.iter().filter(|o|o.options.group.is_none()).map(|o|o.id).collect();
                    for id in ids {self.object_row(ui,&mut s,id);}
                });
            });
        self.items_open = open;
    }
    fn object_row(&mut self, ui: &mut egui::Ui, s: &mut Shared, id: u64) {
        let Some(object) = s.doc.objects.iter().find(|o| o.id == id) else {
            return;
        };
        let label = object.label();
        let mut locked = object.options.locked;
        ui.push_id(id, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut locked, "")
                    .on_hover_text("Locked")
                    .changed()
                    && !s.doc.canvas.locked
                {
                    s.checkpoint();
                    if let Some(o) = s.doc.objects.iter_mut().find(|o| o.id == id) {
                        o.options.locked = locked;
                    }
                    s.changed();
                }
                let response = ui.selectable_label(self.selected.contains(&id), label);
                if response.clicked() {
                    if !ui.input(|i| i.modifiers.ctrl || i.modifiers.shift) {
                        self.selected.clear();
                    }
                    if !self.selected.insert(id) {
                        self.selected.remove(&id);
                    }
                    self.focused = true;
                }
                if response.double_clicked() {
                    self.selected = HashSet::from([id]);
                    self.need_fit = true;
                }
            })
        });
    }
}
