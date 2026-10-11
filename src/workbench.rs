use super::*;
use crate::ui_theme::{self as theme, Icon};
#[derive(Default)]
pub struct State {
    pub palette: bool,
    pub paired_fullscreen: bool,
    pub fullscreen_request: Option<bool>,
    pub recycle: Option<Vec<PathBuf>>,
    query: String,
    focus: bool,
    selection: usize,
    pub layout_dirty: bool,
}
impl App {
    pub fn workspace(&mut self, mode: u8, ctx: &egui::Context) {
        self.settings.review = mode == 2;
        if mode != 2 {
            self.set_dual(mode == 1, ctx);
            self.set_split_preview(mode == 1, ctx);
        }
        self.settings.show_preview = true;
        self.settings.show_tree = mode != 2;
        self.save_preferences();
    }
    fn dual_panes_toggle(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(self.dual_panes, "Dual panes")
            .on_hover_text("Switch between one and two file panes")
            .clicked()
        {
            self.settings.review = false;
            self.set_dual(!self.dual_panes, ui.ctx());
            self.save_preferences();
        }
    }
    pub fn app_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let wide = ui.available_width() > 1000.0;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            theme::paint(ui.painter(), Icon::Compare, rect, theme::ACCENT);
            if wide {
                ui.label(RichText::new("view-calipers").strong());
                ui.add_space(8.0);
            }
            ui.menu_button("Open", |ui| {
                if ui.button("New collection…").clicked() { self.create_board(Vec::new(), &ctx); ui.close_menu(); }
                if ui.button("Folder…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.pending_open = Some(p);
                    }
                    ui.close_menu();
                }
                if ui.button("File…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                        self.pending_open = Some(p);
                    }
                    ui.close_menu();
                }
                if ui.button("This PC").clicked() {
                    self.select(Node::Computer, &ctx);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("New tab").clicked() {
                    self.new_tab(false, &ctx);
                    ui.close_menu();
                }
            });
            self.workspaces_menu(ui);
            self.dual_panes_toggle(ui);
            self.show_navigation(ui);
            let reserve = if wide { 280.0 } else { 220.0 };
            ui.allocate_ui_with_layout(
                Vec2::new(
                    (ui.available_width() - reserve).max(75.0),
                    ui.spacing().interact_size.y,
                ),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| self.file_address(ui),
            );
            ui.menu_button(
                if self.settings.review {
                    "Review"
                } else if self.dual_panes {
                    "Compare"
                } else {
                    "Browse"
                },
                |ui| {
                    for (mode, label, hint) in [
                        (0, "Browse", "Files and a single preview"),
                        (1, "Compare", "Two file panes and two independent previews"),
                        (2, "Review", "Large preview with a filmstrip"),
                    ] {
                        if ui.button(label).on_hover_text(hint).clicked() {
                            self.workspace(mode, &ctx);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let mut dual = self.dual_panes;
                    if ui.checkbox(&mut dual, "Dual panes").changed() {
                        self.settings.review = false;
                        self.set_dual(dual, &ctx);
                        self.save_preferences();
                    }
                    if ui
                        .checkbox(&mut self.settings.show_tree, "Folder sidebar")
                        .changed()
                    {
                        self.save_preferences();
                    }
                    if ui
                        .checkbox(&mut self.settings.show_preview, "Preview panel")
                        .changed()
                    {
                        self.settings.review = false;
                        self.save_preferences();
                    }
                },
            );
            ui.menu_button(
                if wide {
                    if self.settings.comfortable {
                        "Comfortable"
                    } else {
                        "Compact"
                    }
                } else {
                    "Density"
                },
                |ui| {
                    let mut comfortable = self.settings.comfortable;
                    ui.selectable_value(&mut comfortable, false, "Compact");
                    ui.selectable_value(&mut comfortable, true, "Comfortable");
                    if comfortable != self.settings.comfortable {
                        self.settings.comfortable = comfortable;
                        self.apply_preferences(&ctx);
                        self.save_preferences();
                    }
                },
            );
            if theme::button(
                ui,
                Icon::Search,
                "Search commands · Ctrl+Shift+P",
                true,
                false,
            )
            .clicked()
            {
                self.open_palette();
            }
            if theme::button(ui, Icon::Settings, "Settings", true, self.settings_open).clicked() {
                self.open_settings();
            }
        });
    }
    pub fn navigation_icons(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if theme::button(
            ui,
            Icon::Back,
            "Back · Alt+Left",
            self.history_index > 0,
            false,
        )
        .clicked()
        {
            self.navigate_history(-1, &ctx);
        }
        if theme::button(
            ui,
            Icon::Forward,
            "Forward · Alt+Right",
            self.history_index + 1 < self.history.len(),
            false,
        )
        .clicked()
        {
            self.navigate_history(1, &ctx);
        }
        if theme::button(
            ui,
            Icon::Up,
            "Parent folder · Alt+Up",
            !matches!(self.selected, Some(Node::Computer)),
            false,
        )
        .clicked()
        {
            self.navigate_up(&ctx);
        }
        if theme::button(ui, Icon::Refresh, "Refresh", true, false).clicked() {
            self.archive_cache.clear();
            if self.files.search.results{self.submit_search(&ctx);}else if let Some(node) = self.selected.clone() {
                self.select(node, &ctx);
            }
        }
    }
    pub fn pane_header(&mut self, ui: &mut egui::Ui, index: usize, active: bool) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(7.0, 20.0), Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                3.0,
                if active { theme::ACCENT } else { theme::MUTED },
            );
            ui.label(RichText::new(format!("{}", index + 1)).color(if active {
                theme::ACCENT
            } else {
                theme::MUTED
            }));
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| self.show_tabs(ui),
            );
        });
        if self.files.search.results { ui.label(RichText::new("Everything results").color(theme::ACCENT)); }
        let current = self.selected.clone().unwrap_or(Node::Computer);
        let nodes = current.breadcrumbs();
        let mut target = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.set_max_width(ui.available_width());
            if nodes.len() > 3 {
                ui.menu_button("…", |ui| {
                    for node in nodes.iter().take(nodes.len() - 3) {
                        if ui.button(node.label()).clicked() {
                            target = Some(node.clone());
                            ui.close_menu();
                        }
                    }
                });
            }
            let crumb_width =
                ((ui.available_width() - 32.0) / (nodes.len().min(3) as f32)).max(20.0);
            for node in nodes.iter().skip(nodes.len().saturating_sub(3)) {
                if ui
                    .add_sized(
                        Vec2::new(crumb_width, 24.0),
                        egui::Button::new(RichText::new(node.label()).color(if *node == current {
                            theme::TEXT
                        } else {
                            theme::MUTED
                        }))
                        .frame(false)
                        .truncate()
                        .min_size(Vec2::new(0.0, 24.0)),
                    )
                    .on_hover_text(format!("{node:?}"))
                    .clicked()
                {
                    target = Some(node.clone());
                }
                if *node != current {
                    ui.label(RichText::new("›").color(theme::MUTED));
                }
            }
        });
        if let Some(node) = target {
            self.select(node, ui.ctx());
        }
        ui.separator();
        ui.push_id(self.active_tab, |ui| self.show_gallery(ui));
    }
    pub fn styled_tabs(&mut self, ui: &mut egui::Ui) {
        let (mut switch, mut close, mut lock, mut duplicate, mut add) =
            (None, None, None, None, false);
        egui::ScrollArea::horizontal()
            .id_salt("tabs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, tab) in self.tabs.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            let active = index == self.active_tab;
                            let width = (ui.available_width() - 32.0).clamp(80.0, 160.0);
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(width, ui.spacing().interact_size.y),
                                Sense::click(),
                            );
                            if active || response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    5.0,
                                    if active { theme::RAISED } else { theme::HOVER },
                                );
                            }
                            if active {
                                ui.painter().line_segment(
                                    [rect.left_bottom(), rect.right_bottom()],
                                    egui::Stroke::new(2.0_f32, theme::ACCENT),
                                );
                            }
                            let offset = if tab.locked {
                                theme::paint(
                                    ui.painter(),
                                    Icon::Lock,
                                    Rect::from_min_size(
                                        rect.min + Vec2::new(6.0, 6.0),
                                        Vec2::splat(15.0),
                                    ),
                                    theme::MUTED,
                                );
                                26.0
                            } else {
                                8.0
                            };
                            let textrect = Rect::from_min_max(
                                rect.min + Vec2::new(offset, 0.0),
                                rect.max - Vec2::new(6.0, 0.0),
                            );
                            ui.painter().with_clip_rect(textrect).text(
                                textrect.left_center(),
                                Align2::LEFT_CENTER,
                                &tab.title,
                                FontId::proportional(theme::font_size(self.settings.comfortable)),
                                if active { theme::TEXT } else { theme::MUTED },
                            );
                            if response.clicked() {
                                switch = Some(index);
                            }
                            response.clone().on_hover_text(&tab.title);
                            response.context_menu(|ui| {
                                if ui.button("New tab").clicked() {
                                    add = true;
                                    ui.close_menu();
                                }
                                if ui.button("Duplicate tab").clicked() {
                                    duplicate = Some(index);
                                    ui.close_menu();
                                }
                                if ui
                                    .button(if tab.locked { "Unlock tab" } else { "Lock tab" })
                                    .clicked()
                                {
                                    lock = Some(index);
                                    ui.close_menu();
                                }
                                if ui
                                    .add_enabled(
                                        !tab.locked && self.tabs.len() > 1,
                                        egui::Button::new("Close tab"),
                                    )
                                    .clicked()
                                {
                                    close = Some(index);
                                    ui.close_menu();
                                }
                            });
                            if !tab.locked
                                && self.tabs.len() > 1
                                && theme::button(ui, Icon::Close, "Close tab", true, false)
                                    .clicked()
                            {
                                close = Some(index);
                            }
                        });
                    }
                    if theme::button(ui, Icon::Plus, "New tab · Ctrl+T", true, false).clicked() {
                        add = true;
                    }
                });
            });
        if let Some(i) = switch {
            self.switch_tab(i);
        }
        if let Some(i) = lock {
            self.tabs[i].locked = !self.tabs[i].locked;
        }
        if let Some(i) = close {
            self.close_tab(i);
        }
        if let Some(i) = duplicate {
            self.switch_tab(i);
            self.new_tab(true, ui.ctx());
        }
        if add {
            self.new_tab(false, ui.ctx());
        }
    }
    pub fn open_palette(&mut self) {
        self.workbench.palette = true;
        self.workbench.focus = true;
        self.workbench.query.clear();
        self.workbench.selection = 0;
    }
    pub fn palette(&mut self, ctx: &egui::Context) {
        if !self.workbench.palette {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.workbench.palette = false;
            return;
        }
        let matches: Vec<_> = settings::ACTIONS
            .iter()
            .copied()
            .filter(|a| {
                a.label()
                    .to_lowercase()
                    .contains(&self.workbench.query.to_lowercase())
            })
            .collect();
        if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
            self.workbench.selection =
                (self.workbench.selection + 1).min(matches.len().saturating_sub(1));
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
            self.workbench.selection = self.workbench.selection.saturating_sub(1);
        }
        self.workbench.selection = self
            .workbench
            .selection
            .min(matches.len().saturating_sub(1));
        let mut choice = None;
        let mut open = true;
        egui::Window::new("Commands")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(540.0)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 80.0))
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.workbench.query)
                        .hint_text("Search actions…")
                        .desired_width(f32::INFINITY),
                );
                if self.workbench.focus {
                    response.request_focus();
                    self.workbench.focus = false;
                }
                if response.changed() {
                    self.workbench.selection = 0;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        for (i, action) in matches.iter().enumerate() {
                            let binding = self.settings.bindings[*action as usize]
                                .map(settings::Binding::label)
                                .unwrap_or_default();
                            let response = ui.selectable_label(
                                i == self.workbench.selection,
                                format!("{}     {}", action.label(), binding),
                            );
                            if i == self.workbench.selection
                                && ctx.input(|i| {
                                    i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::ArrowDown)
                                })
                            {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                choice = Some(*action);
                            }
                        }
                        if matches.is_empty() {
                            ui.label("No matching commands");
                        }
                    });
                ui.small("Arrow keys to navigate · Enter to run · Esc to close");
            });
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            choice = matches.get(self.workbench.selection).copied();
        }
        if let Some(action) = choice {
            self.workbench.palette = false;
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.run_action(action, ctx);
        }
        if !open {
            self.workbench.palette = false;
        }
    }
    fn run_action(&mut self, action: Action, ctx: &egui::Context) {
        use crate::file_browser::Command;
        match action {
            Action::Next | Action::NextAlternate => self.step(1, ctx),
            Action::Previous | Action::PreviousAlternate => self.step(-1, ctx),
            Action::First => {
                if self.pdf.is_some() {
                    self.load_pdf_page(0, ctx);
                } else {
                    self.goto(0, ctx);
                }
            }
            Action::Last => {
                if let Some((_, _, count)) = self.pdf.clone() {
                    self.load_pdf_page(count - 1, ctx);
                } else if !self.images.is_empty() {
                    self.goto(self.images.len() - 1, ctx);
                }
            }
            Action::Fit => self.fit = true,
            Action::ActualSize => {
                self.fit = false;
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            Action::PlayPause => {
                if let Some(p) = self.video.as_mut() {
                    if p.playing() {
                        p.pause();
                    } else if let Err(e) = p.play(ctx) {
                        self.status = e.to_string();
                    }
                }
            }
            Action::Fullscreen => self.set_fullscreen(ctx, !self.fullscreen),
            Action::Back => self.navigate_history(-1, ctx),
            Action::Forward => self.navigate_history(1, ctx),
            Action::Up => self.navigate_up(ctx),
            Action::Dual => {
                self.settings.review = false;
                self.set_dual(!self.dual_panes, ctx);
                self.save_preferences();
            }
            Action::SwitchPane => self.switch_pane(1 - self.active_pane),
            Action::Settings => self.open_settings(),
            Action::NewTab => self.new_tab(false, ctx),
            Action::CloseTab => self.close_tab(self.active_tab),
            Action::DuplicateTab => self.new_tab(true, ctx),
            Action::LockTab => {
                self.tabs[self.active_tab].locked = !self.tabs[self.active_tab].locked
            }
            Action::CopyFiles => self.files.pending = Some(Command::Copy(false)),
            Action::CutFiles => self.files.pending = Some(Command::Copy(true)),
            Action::PasteFiles => self.files.pending = Some(Command::Paste),
            Action::DuplicateFiles => self.files.pending = Some(Command::Duplicate),
            Action::RenameFile => self.files.pending = Some(Command::Rename),
            Action::DeleteFiles => self.files.pending = Some(Command::Delete),
            Action::Address => self.files.focus_address = true,
            Action::SelectAllFiles => self.select_all_files(),
            Action::DetailsView => self.files.details = !self.files.details,
        }
    }
    pub fn filmstrip(&mut self, ui: &mut egui::Ui) {
        let mut target = None;
        egui::ScrollArea::horizontal()
            .id_salt(("review_strip", self.active_pane))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, image) in self.images.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(106.0, 76.0), Sense::click());
                        ui.painter().rect_filled(
                            rect,
                            4.0,
                            if self.current == Some(i) {
                                theme::SELECTED
                            } else {
                                theme::RAISED
                            },
                        );
                        if let Some(texture) = self.thumbs.get(i).and_then(Option::as_ref) {
                            let bounds = rect.shrink(5.0);
                            let size = texture.size_vec2();
                            let scale = (bounds.width() / size.x).min(bounds.height() / size.y);
                            ui.painter().image(
                                texture.id(),
                                Rect::from_center_size(rect.center(), size * scale),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            theme::paint(
                                ui.painter(),
                                Icon::File,
                                Rect::from_center_size(rect.center(), Vec2::splat(26.0)),
                                theme::MUTED,
                            );
                        }
                        if response.clicked() {
                            target = Some(i);
                        }
                        response.on_hover_text(&image.label);
                    }
                });
            });
        if let Some(i) = target {
            self.goto(i, ui.ctx());
        }
    }
}

#[cfg(test)]
mod pane_toggle_tests {
    use super::*;
    #[test]
    fn toolbar_toggle_switches_both_ways_and_exits_review() {
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.settings.review = true;
        let frame = |app: &mut App, pressed: Option<bool>| {
            let pos = Pos2::new(35.0, 18.0);
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.dual_panes_toggle(ui));
                },
            );
        };
        frame(&mut app, None);
        frame(&mut app, Some(true));
        frame(&mut app, Some(false));
        assert!(app.dual_panes && !app.settings.review);
        frame(&mut app, Some(true));
        frame(&mut app, Some(false));
        assert!(!app.dual_panes && !app.split_preview);
    }
}
