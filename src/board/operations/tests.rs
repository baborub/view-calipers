use super::*;

fn object(id: u64, center: [f32; 2], size: [f32; 2]) -> Object {
    Object {
        id,
        center,
        size,
        angle: 0.0,
        options: Default::default(),
        content: Content::Image {
            source: PathBuf::from(format!("image{id}.png")),
        },
    }
}
fn sample() -> Document {
    let mut d = Document::new("Layout".into());
    d.objects = vec![
        object(1, [0.0, 0.0], [200.0, 100.0]),
        object(2, [0.0, 0.0], [80.0, 320.0]),
        object(3, [0.0, 0.0], [150.0, 150.0]),
    ];
    d.objects[1].angle = 0.7;
    d
}
fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 0.003, "{a} != {b}");
}

#[test]
fn layouts_separate_rotated_objects_and_preserve_locked_objects() {
    for command in [Command::Pack, Command::Grid, Command::Row, Command::Column] {
        let mut d = sample();
        let mut locked = object(4, [5000.0, 5000.0], [40.0, 80.0]);
        locked.options.locked = true;
        d.objects.push(locked.clone());
        assert!(d.apply(&mut HashSet::new(), command, 1.7));
        d.validate().unwrap();
        assert_eq!(d.objects[3], locked);
        for i in 0..3 {
            for j in i + 1..3 {
                assert!(
                    !d.objects[i]
                        .bounds()
                        .expand(d.canvas.gap * 0.49)
                        .intersects(d.objects[j].bounds().expand(d.canvas.gap * 0.49)),
                    "{command:?}"
                );
            }
        }
    }
}
#[test]
fn group_pack_and_normalize_transform_the_group_as_one_unit() {
    let mut d = sample();
    d.objects[1].center = [350.0, 100.0];
    let mut selection = HashSet::from([1, 2]);
    d.apply(&mut selection, Command::Group, 1.0);
    let g = d.objects[0].options.group.unwrap();
    let offset = d.objects[1].center() - d.objects[0].center();
    assert_eq!(d.selection_for(1, false), selection);
    assert_eq!(d.selection_for(1, true), HashSet::from([1]));
    selection.clear();
    d.apply(&mut selection, Command::Pack, 1.0);
    assert!((d.objects[1].center() - d.objects[0].center() - offset).length() < 0.001);
    assert!(!d.group_bounds(g).unwrap().intersects(d.objects[2].bounds()));
    let before = d.objects.clone();
    d.apply(&mut selection, Command::Normalize(0), 1.0);
    near(
        d.group_bounds(g).unwrap().width(),
        d.objects[2].bounds().width(),
    );
    let scale = d.objects[0].size[0] / before[0].size[0];
    near(d.objects[1].size[1] / before[1].size[1], scale);
    near(
        (d.objects[1].center() - d.objects[0].center()).length(),
        offset.length() * scale,
    );
}
#[test]
fn align_and_normalize_keep_aspect_ratio() {
    for axis in 0..3 {
        let mut d = sample();
        let before = d.objects.clone();
        d.apply(&mut HashSet::new(), Command::Normalize(axis), 1.0);
        let metric = |o: &Object| match axis {
            0 => o.bounds().width(),
            1 => o.bounds().height(),
            _ => o.bounds().area().sqrt(),
        };
        for (o, b) in d.objects.iter().zip(&before) {
            near(o.size[0] / o.size[1], b.size[0] / b.size[1]);
            near(metric(o), metric(&d.objects[0]));
        }
        d.apply(&mut HashSet::new(), Command::Align(2), 1.0);
        for o in &d.objects {
            near(o.bounds().right(), d.objects[0].bounds().right());
        }
    }
}
#[test]
fn duplicate_has_independent_group_ids_and_keeps_media_references() {
    let mut d = sample();
    let mut selected = HashSet::from([1, 2]);
    d.apply(&mut selected, Command::Group, 1.0);
    let old = d.clone();
    d.apply(&mut selected, Command::Duplicate, 1.0);
    assert_eq!(d.objects.len(), 5);
    assert_eq!(selected, HashSet::from([4, 5]));
    assert_ne!(d.objects[0].options.group, d.objects[3].options.group);
    assert_eq!(d.objects[3].options.group, d.objects[4].options.group);
    for (copy, original) in d.objects[3..].iter().zip(&old.objects) {
        assert_eq!(copy.content, original.content);
        assert_eq!(copy.center(), original.center() + Vec2::splat(30.0));
    }
    d.apply(&mut selected, Command::Ungroup, 1.0);
    assert_eq!(d.groups.len(), 1);
    assert!(d.objects[0].options.group.is_some());
    d.validate().unwrap();
}
#[test]
fn locked_objects_and_canvas_reject_edits() {
    let mut d = sample();
    let mut selected = HashSet::from([1]);
    d.apply(&mut selected, Command::ToggleLock, 1.0);
    let before = d.objects[0].clone();
    for command in [
        Command::Duplicate,
        Command::FlipX,
        Command::Grayscale,
        Command::Opacity(0.2),
        Command::Pack,
    ] {
        assert!(!d.apply(&mut selected, command, 1.0));
        assert_eq!(d.objects[0], before);
    }
    d.canvas.locked = true;
    let before = d.clone();
    assert!(!d.apply(&mut selected, Command::ToggleLock, 1.0));
    assert_eq!(d, before);
    d.apply(&mut selected, Command::UnlockAll, 1.0);
    assert!(!d.canvas.locked && d.objects.iter().all(|o| !o.options.locked));
}
#[test]
fn crop_composes_with_rotation_flips_and_resets_without_touching_source() {
    for (fx, fy) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut o = object(1, [55.0, -72.0], [400.0, 240.0]);
        o.options.flip_x = fx;
        o.options.flip_y = fy;
        o.angle = 0.71;
        let original = o.clone();
        let crop = Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.8, 0.9));
        let top_left = original.center()
            + rotate(
                (crop.min - Pos2::new(0.5, 0.5)) * original.display_size([1000, 600]),
                original.angle,
            );
        assert!(o.crop_to(crop, [1000, 600]));
        assert!(
            (o.center() + rotate(-o.display_size([1000, 600]) * 0.5, o.angle) - top_left).length()
                < 0.001
        );
        let c = o.options.crop;
        let uv = o.effects(false).uv;
        near(uv[0], if fx { c[2] } else { c[0] });
        near(uv[1], if fy { c[3] } else { c[1] });
        assert!(o.crop_to(
            Rect::from_min_max(Pos2::new(0.2, 0.1), Pos2::new(0.9, 0.7)),
            [1000, 600]
        ));
        o.reset_crop();
        near(o.center[0], original.center[0]);
        near(o.center[1], original.center[1]);
        near(o.size[0], original.size[0]);
        near(o.size[1], original.size[1]);
        assert_eq!(o.options, original.options);
        assert_eq!(o.content, original.content);
    }
}
#[test]
fn repeated_tiny_crop_stops_before_corrupting_saved_geometry() {
    let mut d = sample();
    let mut count = 0;
    while d.objects[0].crop_to(
        Rect::from_min_max(Pos2::ZERO, Pos2::new(0.1, 0.1)),
        [1000, 500],
    ) {
        d.validate().unwrap();
        count += 1;
    }
    assert!(count > 0 && count < 8);
    d.validate().unwrap();
}
#[test]
fn old_boards_default_new_options_and_new_board_round_trips() {
    let root = super::super::tests::fixture("board-tools-format");
    let path = root.join("board.vcboard");
    let mut d = sample();
    let mut json = serde_json::to_value(&d).unwrap();
    json.as_object_mut().unwrap().remove("groups");
    json.as_object_mut().unwrap().remove("canvas");
    for object in json["objects"].as_array_mut().unwrap() {
        object.as_object_mut().unwrap().remove("options");
    }
    for version in [1, 2] {
        json["version"] = version.into();
        let legacy: Document = serde_json::from_value(json.clone()).unwrap();
        legacy.validate().unwrap();
        assert_eq!(legacy.canvas, CanvasOptions::default());
        assert!(legacy
            .objects
            .iter()
            .all(|o| o.options == ObjectOptions::default()));
    }
    let mut selected = HashSet::from([1, 2]);
    d.apply(&mut selected, Command::Group, 1.0);
    d.apply(&mut selected, Command::Grayscale, 1.0);
    d.apply(&mut selected, Command::Opacity(0.45), 1.0);
    d.objects[0].crop_to(
        Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.7, 0.8)),
        [800, 400],
    );
    d.canvas.snap = true;
    d.canvas.grayscale = true;
    d.canvas.step = 40.0;
    d.canvas.grid = false;
    assert_eq!(d.version, 3);
    write(&path, &d).unwrap();
    assert_eq!(read(&path).unwrap(), d);
    let mut invalid = d;
    invalid.objects[0].options.crop = [0.5, 0.0, 0.4, 1.0];
    assert!(invalid.validate().is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn near_id_limit_is_rejected_without_partial_duplication() {
    let mut d = sample();
    d.objects[2].id = u64::MAX - 2;
    let mut selected = HashSet::from([1, 2]);
    let before = d.clone();
    assert!(!d.apply(&mut selected, Command::Duplicate, 1.0));
    assert_eq!(d, before);
}
