use super::*;

/// Pane indices are physical positions, independent of which browser owns focus.
fn pair_areas(rect: Rect, fraction: f32) -> [Rect; 2] {
    let center = rect.left() + rect.width() * fraction.clamp(0.15, 0.85);
    [
        Rect::from_min_max(rect.min, Pos2::new(center - 5.0, rect.bottom())),
        Rect::from_min_max(Pos2::new(center + 5.0, rect.top()), rect.max),
    ]
}
fn file_at_position(track: Rect, x: f32, count: usize) -> usize {
    (((x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0) * count.saturating_sub(1) as f32)
        .round() as usize
}
impl App {
    pub(super) fn process_fullscreen_request(&mut self, ctx: &egui::Context) {
        let requested = self.workbench.fullscreen_request.take().or_else(|| {
            self.inactive
                .as_mut()
                .and_then(|other| other.workbench.fullscreen_request.take())
        });
        if let Some(fullscreen) = requested {
            self.set_fullscreen(ctx, fullscreen);
        }
    }
    pub(super) fn show_paired_fullscreen(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let divider_x = rect.left() + rect.width() * self.settings.fullscreen_fraction;
        let divider = Rect::from_min_max(
            Pos2::new(divider_x - 5.0, rect.top()),
            Pos2::new(divider_x + 5.0, rect.bottom()),
        );
        let response = ui.interact(
            divider,
            ui.id().with("fullscreen_divider"),
            Sense::click_and_drag(),
        );
        if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                self.settings.fullscreen_fraction =
                    ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.15, 0.85);
            }
        }
        if response.double_clicked() {
            self.settings.fullscreen_fraction = 0.5;
            self.save_preferences();
        }
        if response.drag_stopped() {
            self.save_preferences();
        }
        response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        let areas = pair_areas(rect, self.settings.fullscreen_fraction);
        let divider_x = (areas[0].right() + areas[1].left()) * 0.5;
        if let Some(pos) = ui.ctx().input(|i| {
            if i.pointer.any_pressed() || i.raw_scroll_delta.y != 0.0 {
                i.pointer.hover_pos()
            } else {
                None
            }
        }) {
            for (index, area) in areas.iter().enumerate() {
                if area.contains(pos) {
                    self.switch_pane(index);
                }
            }
        }
        ui.painter().line_segment(
            [
                Pos2::new(divider_x, rect.top()),
                Pos2::new(divider_x, rect.bottom()),
            ],
            egui::Stroke::new(1.0_f32, ui_theme::BORDER),
        );
        for (index, area) in areas.into_iter().enumerate() {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(area), |ui| {
                ui.set_clip_rect(area.intersect(ui.clip_rect()));
                ui.push_id(("paired_fullscreen", index), |ui| {
                    egui::Frame::none().inner_margin(8.0).show(ui, |ui| {
                        if index == self.active_pane {
                            self.show_preview_content(ui, index);
                        } else if let Some(other) = self.inactive.as_mut() {
                            other.settings = self.settings.clone();
                            other.show_preview_content(ui, index);
                        }
                    });
                });
            });
        }
    }

    pub(super) fn fullscreen_file_slider(&mut self, ui: &mut egui::Ui) {
        let media=self.media_navigation();let count = media.len();
        ui.horizontal(|ui| {
            ui.small("Files");
            ui.small(format!("{} / {count}", self.current.and_then(|i|media.iter().position(|&m|m==i)).map_or(0,|i|i+1)));
        });
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 28.0),
            Sense::click_and_drag(),
        );
        let track = rect.shrink2(Vec2::new(8.0, 0.0));
        let y = rect.center().y;
        let current=self.current.and_then(|i|media.iter().position(|&m|m==i)).unwrap_or(0);
        let fraction = if count > 1 {
            current as f32 / (count - 1) as f32
        } else {
            0.0
        };
        let x = track.left() + fraction * track.width();
        ui.painter().line_segment(
            [Pos2::new(track.left(), y), Pos2::new(track.right(), y)],
            egui::Stroke::new(4.0_f32, ui_theme::BORDER),
        );
        ui.painter().line_segment(
            [Pos2::new(track.left(), y), Pos2::new(x, y)],
            egui::Stroke::new(4.0_f32, ui_theme::SELECTED),
        );
        ui.painter()
            .circle_filled(Pos2::new(x, y), 6.0, Color32::LIGHT_BLUE);
        if count == 0 {
            return;
        }
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let target = file_at_position(track, pos.x, count);
                if self.current != Some(media[target]) {
                    self.goto(media[target], ui.ctx());
                }
            }
        }
        if response.hovered() || response.dragged() {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let target = file_at_position(track, pos.x, count);
                if self.thumbs.get(media[target]).and_then(Option::as_ref).is_none() {
                    if let Some(queue) = &self.thumb_queue {
                        let mut queue = queue.lock().unwrap();
                        if let Some(index) = queue.iter().position(|&i| i == target) {
                            queue.remove(index);
                            queue.push_front(media[target]);
                        }
                    }
                }
                let screen = ui.ctx().screen_rect();
                let popup_x = (pos.x - 110.0)
                    .clamp(screen.left(), (screen.right() - 220.0).max(screen.left()));
                egui::Area::new(ui.id().with("file_hover_thumb"))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .fixed_pos(Pos2::new(popup_x, (rect.top() - 190.0).max(screen.top())))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_width(200.0);
                            if let Some(texture) = self.thumbs.get(media[target]).and_then(Option::as_ref)
                            {
                                ui.add(
                                    egui::Image::new(texture).fit_to_exact_size(
                                        texture.size_vec2()
                                            * (200.0 / texture.size_vec2().x)
                                                .min(140.0 / texture.size_vec2().y),
                                    ),
                                );
                            } else {
                                ui.allocate_space(Vec2::new(200.0, 140.0));
                                ui.label("Loading thumbnail...");
                            }
                            ui.small(format!("{} / {count}", target + 1));
                            ui.add(egui::Label::new(&self.images[media[target]].label).truncate());
                        });
                    });
            }
        }
    }
    pub(super) fn preview_wheel(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let (scroll, ctrl, hovered) = ui.input(|i| {
            (
                i.raw_scroll_delta.y,
                i.modifiers.ctrl,
                i.pointer.hover_pos().is_some_and(|pos| area.contains(pos)),
            )
        });
        if !hovered {
            return;
        }
        if ctrl {
            self.wheel_pending = 0.0;
            return;
        }
        if scroll == 0.0 {
            return;
        }
        // Consume only the hovered preview's wheel, leaving gallery/tree scrolling alone.
        ui.ctx().input_mut(|i| {
            i.raw_scroll_delta.y = 0.0;
            i.smooth_scroll_delta.y = 0.0;
        });
        if self.wheel_pending.signum() != scroll.signum() {
            self.wheel_pending = 0.0;
        }
        self.wheel_pending += scroll;
        if self.wheel_pending.abs() < 20.0 {
            return;
        }
        let direction = if self.wheel_pending < 0.0 { 1 } else { -1 };
        self.wheel_pending = 0.0;
        let media=self.media_navigation();
        if !media.is_empty() {
            let current=self.current.and_then(|i|media.iter().position(|&m|m==i));
            let next = if let Some(current) = current {
                (current as isize + direction).rem_euclid(media.len() as isize) as usize
            } else if direction > 0 {0} else {media.len()-1};
            self.goto(media[next], ui.ctx());
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_slider_click_and_drag_browse_only_its_own_folder() {
        let (mut app, ctx, root) = fixture();
        let frame = |app: &mut App, pos: Pos2, press: Option<bool>| {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = press {
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
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_space(500.0);
                        app.fullscreen_file_slider(ui);
                    });
                },
            );
        };
        frame(&mut app, Pos2::new(400.0, 540.0), None);
        frame(&mut app, Pos2::new(400.0, 540.0), Some(true));
        frame(&mut app, Pos2::new(400.0, 540.0), Some(false));
        assert_eq!(app.current, Some(1));
        frame(&mut app, Pos2::new(400.0, 540.0), Some(true));
        frame(&mut app, Pos2::new(785.0, 540.0), None);
        assert_eq!(app.current, Some(2));
        assert_eq!(app.inactive.as_ref().unwrap().current, Some(2));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fullscreen_divider_drags_and_slider_mapping_handles_edges() {
        let (mut app, ctx, root) = fixture();
        app.settings.compare_fullscreen = true;
        app.set_fullscreen(&ctx, true);
        let frame = |app: &mut App, pos: Pos2, pressed: bool| {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
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
                    egui::CentralPanel::default().show(ctx, |ui| app.show_paired_fullscreen(ui));
                },
            );
        };
        frame(&mut app, Pos2::new(400.0, 300.0), false);
        frame(&mut app, Pos2::new(400.0, 300.0), true);
        frame(&mut app, Pos2::new(560.0, 300.0), false);
        assert!((app.settings.fullscreen_fraction - 0.7).abs() < 0.02);
        let track = Rect::from_min_max(Pos2::new(10.0, 0.0), Pos2::new(110.0, 20.0));
        assert_eq!(file_at_position(track, -100.0, 5), 0);
        assert_eq!(file_at_position(track, 60.0, 5), 2);
        assert_eq!(file_at_position(track, 1000.0, 5), 4);
        assert_eq!(file_at_position(track, 60.0, 1), 0);
        assert_eq!(
            settings::Settings::parse("fullscreen_fraction=NaN").fullscreen_fraction,
            0.5
        );
        assert_eq!(
            settings::Settings::parse("fullscreen_fraction=99").fullscreen_fraction,
            0.85
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    fn fixture() -> (App, egui::Context, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "view-calipers-paired-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        for n in 0..3 {
            image::RgbaImage::from_pixel(16, 24, image::Rgba([n * 60, 40, 100, 255]))
                .save(root.join(format!("image{n}.png")))
                .unwrap();
        }
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.cache_thumbnails = false;
        app.select(Node::Dir(root.clone()), &ctx);
        app.set_split_preview(true, &ctx);
        app.goto(0, &ctx); app.wait_photo(&ctx);
        app.inactive.as_mut().unwrap().goto(2, &ctx);app.inactive.as_mut().unwrap().wait_photo(&ctx);
        (app, ctx, root)
    }
    #[test]
    fn fullscreen_from_second_preview_retains_left_right_identity_and_restores_layout() {
        let (mut app, ctx, root) = fixture();
        app.settings.compare_fullscreen = true;
        app.inactive.as_mut().unwrap().workbench.fullscreen_request = Some(true);
        app.process_fullscreen_request(&ctx);
        assert!(
            app.fullscreen
                && app.workbench.paired_fullscreen
                && app.inactive.as_ref().unwrap().fullscreen
        );
        app.switch_pane(1);
        assert_eq!(app.current, Some(2));
        assert_eq!(app.inactive.as_ref().unwrap().current, Some(0));
        let areas = pair_areas(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0)),
            0.5,
        );
        assert!(areas[0].max.x < areas[1].min.x);
        assert_eq!(areas[0].height(), 1080.0);
        app.inactive.as_mut().unwrap().workbench.fullscreen_request = Some(false);
        app.process_fullscreen_request(&ctx);
        assert!(
            !app.fullscreen
                && !app.workbench.paired_fullscreen
                && !app.inactive.as_ref().unwrap().fullscreen
        );
        assert!(app.split_preview && app.dual_panes);
        app.settings.compare_fullscreen = false;
        app.set_fullscreen(&ctx, true);
        assert!(!app.workbench.paired_fullscreen);
        app.settings.compare_fullscreen = true;
        app.set_fullscreen(&ctx, true);
        app.set_dual(false, &ctx);
        assert!(!app.workbench.paired_fullscreen && !app.inactive.as_ref().unwrap().fullscreen);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn wheel_browses_files_ctrl_wheel_zooms_and_other_pane_stays_unchanged() {
        let (mut app, ctx, root) = fixture();
        let frame = |app: &mut App, delta: f32, ctrl: bool| {
            let modifiers = egui::Modifiers {
                ctrl,
                ..Default::default()
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    modifiers,
                    events: vec![
                        egui::Event::PointerMoved(Pos2::new(400.0, 300.0)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: Vec2::new(0.0, delta),
                            modifiers,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.show_image(ui));
                },
            );
        };
        frame(&mut app, -120.0, false);
        assert_eq!(app.current, Some(1));
        assert_eq!(app.inactive.as_ref().unwrap().current, Some(2));
        frame(&mut app, 120.0, false);
        assert_eq!(app.current, Some(0));
        let zoom = app.zoom;
        frame(&mut app, 120.0, true);
        assert_eq!(app.current, Some(0));
        assert!(app.zoom > zoom);
        assert!(!app.fit);
        frame(&mut app, -120.0, true);
        assert!(app.zoom < zoom * 1.1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn double_click_lower_preview_opens_the_root_viewport_for_both_files() {
        let (mut app, ctx, root) = fixture();
        app.settings.compare_fullscreen = true;
        let pos = Pos2::new(400.0, 480.0);
        for (time, pressed) in [(1.0, true), (1.03, false), (1.1, true), (1.13, false)] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    time: Some(time),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.show_preview_panel(ui));
                    app.process_fullscreen_request(ctx);
                },
            );
        }
        assert!(app.fullscreen && app.workbench.paired_fullscreen);
        assert_eq!(app.active_pane, 1);
        assert_eq!(app.current, Some(2));
        assert_eq!(app.inactive.as_ref().unwrap().current, Some(0));
        app.set_fullscreen(&ctx, false);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn paired_preference_is_saved_and_defaults_to_single_fullscreen() {
        let mut s = settings::Settings::default();
        assert!(!s.compare_fullscreen);
        s.compare_fullscreen = true;
        assert!(settings::Settings::parse(&s.text()).compare_fullscreen);
    }
}
