//! Shared visual tokens and resolution-independent toolbar icons.
use eframe::egui::*;
pub const BACKGROUND: Color32 = Color32::from_rgb(17, 19, 23);
pub const PANEL: Color32 = Color32::from_rgb(25, 28, 33);
pub const RAISED: Color32 = Color32::from_rgb(34, 38, 45);
pub const HOVER: Color32 = Color32::from_rgb(45, 51, 60);
pub const BORDER: Color32 = Color32::from_rgb(53, 60, 70);
pub const TEXT: Color32 = Color32::from_rgb(231, 235, 241);
pub const MUTED: Color32 = Color32::from_rgb(158, 169, 183);
pub const ACCENT: Color32 = Color32::from_rgb(116, 184, 241);
pub const SELECTED: Color32 = Color32::from_rgb(35, 66, 92);
pub fn row_height(comfortable: bool) -> f32 {
    if comfortable {
        34.0
    } else {
        26.0
    }
}
pub fn font_size(comfortable: bool) -> f32 {
    if comfortable {
        15.0
    } else {
        14.0
    }
}
pub fn apply(ctx: &Context, comfortable: bool) {
    let mut style = (*ctx.style()).clone();
    let body = font_size(comfortable);
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(body));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(body));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(19.0));
    style.spacing.item_spacing = vec2(
        if comfortable { 8.0 } else { 6.0 },
        if comfortable { 8.0 } else { 4.0 },
    );
    style.spacing.button_padding = vec2(8.0, if comfortable { 6.0 } else { 4.0 });
    style.spacing.interact_size = vec2(26.0, if comfortable { 34.0 } else { 26.0 });
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.spacing.scroll.foreground_color = true;
    style.spacing.scroll.bar_width = if comfortable { 12.0 } else { 10.0 };
    style.spacing.scroll.handle_min_length = 36.0;
    style.spacing.scroll.bar_inner_margin = 5.0;
    style.spacing.scroll.floating_width = 5.0;
    style.spacing.scroll.floating_allocated_width = 10.0;
    style.spacing.scroll.dormant_handle_opacity = 0.3;
    style.spacing.scroll.active_handle_opacity = 0.55;
    style.spacing.scroll.interact_handle_opacity = 0.9;
    style.spacing.window_margin = Margin::same(14.0);
    let v = &mut style.visuals;
    *v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BACKGROUND;
    v.faint_bg_color = RAISED;
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.window_rounding = Rounding::same(8.0);
    v.menu_rounding = Rounding::same(6.0);
    for widget in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        widget.rounding = Rounding::same(5.0);
        widget.fg_stroke = Stroke::new(1.0_f32, TEXT);
        widget.bg_stroke = Stroke::new(1.0_f32, Color32::TRANSPARENT);
    }
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.active.weak_bg_fill = SELECTED;
    v.widgets.open.weak_bg_fill = SELECTED;
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.active.bg_fill = SELECTED;
    v.widgets.open.bg_fill = SELECTED;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, MUTED);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    ctx.set_style(style);
}
pub fn install_fonts(ctx: &Context) {
    let windows = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:/Windows".into());
    if let Ok(bytes) = std::fs::read(windows.join("Fonts/segoeui.ttf")) {
        let mut fonts = FontDefinitions::default();
        fonts
            .font_data
            .insert("system-ui".into(), FontData::from_owned(bytes));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "system-ui".into());
        ctx.set_fonts(fonts);
    }
}
#[derive(Clone, Copy)]
pub enum Icon {
    Back,
    Forward,
    Up,
    Refresh,
    Folder,
    File,
    Grid,
    List,
    Compare,
    Preview,
    Settings,
    Search,
    Close,
    Plus,
    Lock,
    Copy,
    Move,
    Play,
    Pause,
    More,
    NewFolder,
    Sort,
    Columns,
    Filter,
}
pub fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    let rect = Rect::from_center_size(rect.center(), Vec2::splat(rect.width().min(rect.height())));
    let p = |x: f32, y: f32| {
        pos2(
            rect.left() + rect.width() * x / 24.0,
            rect.top() + rect.height() * y / 24.0,
        )
    };
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|(x, y)| p(*x, *y)).collect(),
            Stroke::new(1.65_f32, color),
        ));
    };
    let box_ = |x: f32, y: f32, w: f32, h: f32| {
        painter.rect_stroke(
            Rect::from_min_max(p(x, y), p(x + w, y + h)),
            2.0,
            Stroke::new(1.5_f32, color),
        );
    };
    match icon {
        Icon::More => {
            for x in [5.0, 12.0, 19.0] {
                painter.circle_filled(p(x, 12.0), 1.5, color);
            }
        }
        Icon::NewFolder => {
            line(&[
                (3., 20.),
                (3., 5.),
                (10., 5.),
                (13., 8.),
                (21., 8.),
                (21., 20.),
                (3., 20.),
            ]);
            line(&[(9., 14.), (17., 14.)]);
            line(&[(13., 10.), (13., 18.)]);
        }
        Icon::Sort => {
            line(&[(7., 4.), (7., 20.)]);
            line(&[(3., 16.), (7., 20.), (11., 16.)]);
            line(&[(13., 5.), (22., 5.)]);
            line(&[(13., 11.), (19., 11.)]);
            line(&[(13., 17.), (16., 17.)]);
        }
        Icon::Columns => {
            box_(3., 4., 18., 16.);
            line(&[(9., 4.), (9., 20.)]);
            line(&[(15., 4.), (15., 20.)]);
        }
        Icon::Filter => {
            line(&[
                (3., 5.),
                (21., 5.),
                (15., 12.),
                (15., 19.),
                (9., 21.),
                (9., 12.),
                (3., 5.),
            ]);
        }
        Icon::Back => {
            line(&[(14., 5.), (7., 12.), (14., 19.)]);
            line(&[(7., 12.), (21., 12.)]);
        }
        Icon::Forward => {
            line(&[(10., 5.), (17., 12.), (10., 19.)]);
            line(&[(3., 12.), (17., 12.)]);
        }
        Icon::Up => {
            line(&[(5., 11.), (12., 4.), (19., 11.)]);
            line(&[(12., 4.), (12., 21.)]);
        }
        Icon::Refresh => {
            line(&[
                (19., 8.),
                (16., 4.),
                (8., 4.),
                (4., 8.),
                (4., 16.),
                (8., 20.),
                (16., 20.),
                (20., 16.),
            ]);
            line(&[(19., 2.), (19., 8.), (13., 8.)]);
        }
        Icon::Folder => {
            line(&[
                (3., 20.),
                (3., 5.),
                (10., 5.),
                (13., 8.),
                (21., 8.),
                (21., 20.),
                (3., 20.),
            ]);
            line(&[(3., 10.), (21., 10.)]);
        }
        Icon::File => {
            line(&[
                (6., 3.),
                (14., 3.),
                (19., 8.),
                (19., 21.),
                (6., 21.),
                (6., 3.),
            ]);
            line(&[(14., 3.), (14., 8.), (19., 8.)]);
        }
        Icon::Grid => {
            for x in [4., 14.] {
                for y in [4., 14.] {
                    box_(x, y, 6., 6.);
                }
            }
        }
        Icon::List => {
            for y in [5., 11., 17.] {
                box_(3., y, 3., 3.);
                line(&[(10., y + 1.5), (21., y + 1.5)]);
            }
        }
        Icon::Compare => {
            box_(3., 4., 18., 16.);
            line(&[(12., 4.), (12., 20.)]);
        }
        Icon::Preview => {
            box_(3., 4., 18., 16.);
            line(&[(15., 4.), (15., 20.)]);
        }
        Icon::Settings => {
            painter.circle_stroke(
                p(12., 12.),
                rect.width() * 0.25,
                Stroke::new(1.6_f32, color),
            );
            painter.circle_stroke(
                p(12., 12.),
                rect.width() * 0.09,
                Stroke::new(1.5_f32, color),
            );
            for n in 0..8 {
                let a = n as f32 * std::f32::consts::TAU / 8.0;
                line(&[
                    (12. + a.cos() * 7., 12. + a.sin() * 7.),
                    (12. + a.cos() * 10., 12. + a.sin() * 10.),
                ]);
            }
        }
        Icon::Search => {
            painter.circle_stroke(
                p(10., 10.),
                rect.width() * 0.27,
                Stroke::new(1.7_f32, color),
            );
            line(&[(15., 15.), (21., 21.)]);
        }
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(18., 6.), (6., 18.)]);
        }
        Icon::Plus => {
            line(&[(12., 5.), (12., 19.)]);
            line(&[(5., 12.), (19., 12.)]);
        }
        Icon::Lock => {
            box_(5., 10., 14., 11.);
            line(&[
                (8., 10.),
                (8., 6.),
                (10., 3.),
                (14., 3.),
                (16., 6.),
                (16., 10.),
            ]);
        }
        Icon::Copy => {
            box_(8., 8., 12., 13.);
            line(&[(16., 8.), (16., 3.), (3., 3.), (3., 16.), (8., 16.)]);
        }
        Icon::Move => {
            line(&[(3., 12.), (21., 12.)]);
            line(&[(15., 6.), (21., 12.), (15., 18.)]);
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![p(7., 4.), p(20., 12.), p(7., 20.)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for x in [6., 15.] {
                painter.rect_filled(Rect::from_min_max(p(x, 4.), p(x + 3., 20.)), 1.0, color);
            }
        }
    }
}
pub fn button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool, selected: bool) -> Response {
    let side = ui.spacing().interact_size.y;
    let response = ui.add_enabled(
        enabled,
        Button::new("")
            .min_size(Vec2::splat(side))
            .fill(if selected {
                SELECTED
            } else {
                Color32::TRANSPARENT
            }),
    );
    let color = if !enabled {
        MUTED.gamma_multiply(0.4)
    } else if selected {
        ACCENT
    } else {
        TEXT
    };
    if enabled && response.hovered() && !selected {
        ui.painter().rect_filled(response.rect, 5.0, HOVER);
    }
    paint(
        ui.painter(),
        icon,
        Rect::from_center_size(response.rect.center(), Vec2::splat(18.0)),
        color,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// Consistent vector icon for popup tools, with the same hit target as toolbar buttons.
pub fn icon_menu<R>(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<Option<R>> {
    let side = ui.spacing().interact_size.y;
    let result = egui::menu::menu_custom_button(
        ui,
        Button::new("")
            .min_size(Vec2::splat(side))
            .fill(if selected {
                SELECTED
            } else {
                Color32::TRANSPARENT
            }),
        contents,
    );
    if result.response.hovered() && !selected {
        ui.painter().rect_filled(result.response.rect, 5.0, HOVER);
    }
    paint(
        ui.painter(),
        icon,
        Rect::from_center_size(result.response.rect.center(), Vec2::splat(18.0)),
        if selected { ACCENT } else { TEXT },
    );
    result
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    InnerResponse {
        response: result.response.on_hover_text(label),
        inner: result.inner,
    }
}

#[cfg(test)]
mod compact_controls_tests {
    use crate::*;
    #[test]
    fn controls_fit_two_rows_at_narrow_and_wide_sizes_in_both_densities() {
        for comfortable in [false, true] {
            for width in [
                180.0, 220.0, 250.0, 300.0, 360.0, 440.0, 520.0, 700.0, 1000.0,
            ] {
                for details in [false, true] {
                    for advanced in [false, true] {
                        let ctx = egui::Context::default();
                        super::apply(&ctx, comfortable);
                        let mut app = App::new(None);
                        app.settings.comfortable = comfortable;
                        app.files.details = details;
                        if advanced {
                            app.files.collection = Some("fixture".into());
                            app.files.collection_custom = true;
                            app.files.search.scope = 2;
                            app.files.search.kind = 3;
                        }
                        let mut measured = egui::Rect::NOTHING;
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 400.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default()
                                    .frame(egui::Frame::none())
                                    .show(ctx, |ui| {
                                        measured = ui
                                            .scope(|ui| {
                                                app.file_toolbar(ui);
                                                app.search_bar(ui);
                                            })
                                            .response
                                            .rect;
                                    });
                            },
                        );
                        assert!(measured.right()<=width+1.0,"Controls overflow: width={width}, comfortable={comfortable}, details={details}, actual={measured:?}");
                        assert!(
                            measured.height() <= if comfortable { 100.0 } else { 82.0 },
                            "Controls exceed two rows: {measured:?}"
                        );
                    }
                }
            }
        }
    }
}
/// Keep the full name in a single line. Only the panel's visible edge clips it;
/// resizing the sidebar reveals more text without a horizontal scroll track.
pub fn sidebar_label(ui:&mut egui::Ui,selected:bool,text:impl Into<egui::WidgetText>)->egui::Response {
    let padding=ui.spacing().button_padding;
    let galley=text.into().into_galley(ui,Some(egui::TextWrapMode::Extend),f32::INFINITY,egui::TextStyle::Button);
    let size=egui::vec2((galley.size().x+padding.x*2.0).min(ui.available_width()).max(1.0),(galley.size().y+padding.y*2.0).max(ui.spacing().interact_size.y));
    let (rect,response)=ui.allocate_exact_size(size,egui::Sense::click());
    let visuals=ui.style().interact_selectable(&response,selected);
    let painter=ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if selected||response.hovered()||response.has_focus(){painter.rect(rect,visuals.rounding,visuals.weak_bg_fill,visuals.bg_stroke);}
    painter.galley(rect.min+padding,galley,visuals.text_color());
    response
}
