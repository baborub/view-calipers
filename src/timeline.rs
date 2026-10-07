use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use crate::video::PlaybackRange;

#[derive(Default)]
pub struct Action {
    pub seek: Option<f64>,
    pub seeking: bool,
    pub range_committed: bool,
}

pub fn timecode(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}.{:03}", millis / 60_000, millis / 1000 % 60, millis % 1000)
}

pub fn time_at(x: f32, rect: Rect, duration: f64) -> f64 {
    (((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64 * duration).clamp(0.0, duration)
}

pub fn show(ui: &mut egui::Ui, position: &mut f64, duration: f64, range: &mut PlaybackRange) -> Action {
    let mut action = Action::default();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 86.0), Sense::hover());
    let track = Rect::from_min_max(rect.min + Vec2::new(12.0, 27.0), Pos2::new(rect.right() - 12.0, rect.top() + 33.0));
    let x_at = |time: f64| track.left() + (time / duration).clamp(0.0, 1.0) as f32 * track.width();
    let seek_rect = Rect::from_min_max(Pos2::new(track.left(), rect.top() + 17.0), Pos2::new(track.right(), rect.top() + 42.0));
    let response = ui.interact(seek_rect, ui.id().with("timeline_seek"), Sense::click_and_drag());
    if response.clicked() || response.dragged() || response.drag_stopped() {
        if let Some(pointer) = response.interact_pointer_pos() {
            *position = time_at(pointer.x, track, duration);
            action.seeking = response.dragged();
            if response.clicked() || response.drag_stopped() { action.seek = Some(*position); }
        }
    }
    let painter = ui.painter();
    painter.text(rect.left_top(), Align2::LEFT_TOP, timecode(*position), FontId::monospace(12.0), Color32::WHITE);
    painter.text(rect.right_top(), Align2::RIGHT_TOP, timecode(duration), FontId::monospace(12.0), Color32::GRAY);
    painter.rect_filled(track, 3.0, Color32::from_gray(65));
    painter.rect_filled(Rect::from_min_max(track.min, Pos2::new(x_at(*position), track.bottom())), 3.0, Color32::from_rgb(95, 170, 220));
    painter.circle_filled(Pos2::new(x_at(*position), track.center().y), 6.0, Color32::WHITE);
    let ticks = ((track.width() / 110.0) as usize).clamp(2, 10);
    for tick in 0..=ticks {
        let x = track.left() + track.width() * tick as f32 / ticks as f32;
        painter.line_segment([Pos2::new(x, track.bottom()), Pos2::new(x, track.bottom() + 4.0)], Stroke::new(1.0_f32, Color32::GRAY));
    }
    let range_rect = Rect::from_min_max(Pos2::new(x_at(range.start), rect.top() + 57.0), Pos2::new(x_at(range.end), rect.top() + 63.0));
    painter.rect_filled(range_rect, 2.0, if range.enabled { Color32::from_rgb(65, 140, 105) } else { Color32::from_gray(50) });
    for is_start in [true, false] {
        let time = if is_start { range.start } else { range.end };
        let x = x_at(time);
        let handle = Rect::from_center_size(Pos2::new(x, rect.top() + 58.0), Vec2::new(20.0, 28.0));
        let response = ui.interact(handle, ui.id().with(if is_start { "marker_a" } else { "marker_b" }), Sense::click_and_drag());
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let time = time_at(pointer.x, track, duration);
                if is_start { range.set_start(time, duration); } else { range.set_end(time, duration); }
            }
        }
        action.range_committed |= response.drag_stopped();
        let color = if is_start { Color32::from_rgb(90, 210, 150) } else { Color32::from_rgb(245, 175, 85) };
        ui.painter().rect_filled(handle.shrink(2.0), 3.0, color);
        ui.painter().text(handle.center(), Align2::CENTER_CENTER, if is_start { "A" } else { "B" }, FontId::monospace(13.0), Color32::BLACK);
        response.on_hover_text(format!("{}: {} — drag the marker", if is_start { "Start" } else { "End" }, timecode(time)));
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_timeline_maps_entire_width_to_clip_duration() {
        for width in [260.0, 960.0, 1920.0] {
            let track = Rect::from_min_size(Pos2::new(12.0, 0.0), Vec2::new(width - 24.0, 6.0));
            assert_eq!(time_at(track.left(), track, 1500.0), 0.0);
            assert_eq!(time_at(track.right(), track, 1500.0), 1500.0);
            assert!((time_at(track.center().x, track, 1500.0) - 750.0).abs() < 0.001);
        }
    }

    #[test]
    fn marker_drag_sets_range_and_commits_on_release() {
        let ctx = egui::Context::default();
        let mut range = PlaybackRange::new(10.0);
        let mut position = 0.0;
        let mut marker = Pos2::ZERO;
        let mut target = Pos2::ZERO;
        let mut committed = false;
        for stage in 0..4 {
            let events = match stage {
                1 => vec![egui::Event::PointerMoved(marker), egui::Event::PointerButton { pos: marker, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::default() }],
                2 => vec![egui::Event::PointerMoved(target)],
                3 => vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::default() }],
                _ => vec![],
            };
            let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 160.0))), events, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    if stage == 0 { marker = rect.min + Vec2::new(12.0, 58.0); target = Pos2::new(rect.center().x, marker.y); }
                    committed |= show(ui, &mut position, 10.0, &mut range).range_committed;
                });
            });
        }
        assert!(range.enabled);
        assert!((range.start - 5.0).abs() < 0.1);
        assert_eq!(range.end, 10.0);
        assert!(committed);
    }
}
