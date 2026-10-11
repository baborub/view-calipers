use super::*;

fn fixture(tag: &str) -> (PathBuf, Editor, egui::Context) {
    let root = crate::board::tests::fixture(tag);
    let path = root.join("board.vcboard");
    let mut doc = Document::new("Tools".into());
    for (id, x) in [(1, -150.0), (2, 150.0)] {
        doc.objects.push(Object {
            id,
            center: [x, 0.0],
            size: [120.0, 90.0],
            angle: 0.0,
            options: Default::default(),
            content: Content::Image {
                source: PathBuf::from(format!("missing-{id}.png")),
            },
        });
    }
    write(&path, &doc).unwrap();
    let mut editor = Editor::open(&path).unwrap();
    let ctx = egui::Context::default();
    frame(&mut editor, &ctx, vec![]);
    editor.focused = true;
    (root, editor, ctx)
}
fn frame(editor: &mut Editor, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| editor.show(ui));
        },
    )
}
fn key(editor: &mut Editor, ctx: &egui::Context, key: Key, modifiers: egui::Modifiers) {
    frame(
        editor,
        ctx,
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
    );
    frame(
        editor,
        ctx,
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }],
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
        vec![egui::Event::PointerMoved(start + Vec2::splat(9.0))],
    );
    frame(editor, ctx, vec![egui::Event::PointerMoved(end)]);
    frame(editor, ctx, vec![button(end, false)]);
}
#[test]
fn grouping_duplication_and_redo_shortcuts_change_only_the_focused_board() {
    let (root, mut editor, ctx) = fixture("reference-shortcuts");
    editor.selected = HashSet::from([1, 2]);
    key(&mut editor, &ctx, Key::G, egui::Modifiers::CTRL);
    assert_eq!(editor.shared.lock().unwrap().doc.groups.len(), 1);
    key(
        &mut editor,
        &ctx,
        Key::G,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    assert!(
        editor.shared.lock().unwrap().doc.groups.is_empty(),
        "Ctrl+Shift+G must ungroup, not regroup"
    );
    key(&mut editor, &ctx, Key::Z, egui::Modifiers::CTRL);
    assert_eq!(editor.shared.lock().unwrap().doc.groups.len(), 1);
    key(
        &mut editor,
        &ctx,
        Key::Z,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    assert!(
        editor.shared.lock().unwrap().doc.groups.is_empty(),
        "Ctrl+Shift+Z must redo"
    );
    key(&mut editor, &ctx, Key::D, egui::Modifiers::CTRL);
    assert_eq!(editor.shared.lock().unwrap().doc.objects.len(), 4);
    editor.focused = false;
    key(&mut editor, &ctx, Key::D, egui::Modifiers::CTRL);
    assert_eq!(editor.shared.lock().unwrap().doc.objects.len(), 4);
    drop(editor);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn group_drag_moves_both_members_and_lock_protects_them() {
    let (root, mut editor, ctx) = fixture("reference-drag");
    editor.selected = HashSet::from([1, 2]);
    key(&mut editor, &ctx, Key::G, egui::Modifiers::CTRL);
    editor.selected.clear();
    let before = editor.shared.lock().unwrap().doc.clone();
    let p = before
        .camera
        .screen(before.objects[0].center(), editor.area);
    drag(&mut editor, &ctx, p, p + Vec2::new(64.0, 32.0));
    let after = editor.shared.lock().unwrap().doc.clone();
    assert_eq!(editor.selected, HashSet::from([1, 2]));
    for (a, b) in after.objects.iter().zip(&before.objects) {
        assert!(
            (a.center() - b.center() - Vec2::new(64.0, 32.0) / after.camera.zoom).length() < 0.01
        );
    }
    key(&mut editor, &ctx, Key::L, egui::Modifiers::ALT);
    let before = editor.shared.lock().unwrap().doc.clone();
    let p = before
        .camera
        .screen(before.objects[0].center(), editor.area);
    drag(&mut editor, &ctx, p, p + Vec2::new(56.0, 17.0));
    assert_eq!(editor.shared.lock().unwrap().doc.objects, before.objects);
    drop(editor);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn crop_drag_undo_and_focus_return_are_stable() {
    let (root, mut editor, ctx) = fixture("reference-crop");
    editor.media_sizes.insert(1, [400, 300]);
    let before = editor.shared.lock().unwrap().doc.clone();
    let o = &before.objects[0];
    let start = before
        .camera
        .screen(o.center() - Vec2::new(45.0, 30.0), editor.area);
    let end = before
        .camera
        .screen(o.center() + Vec2::new(30.0, 20.0), editor.area);
    key(&mut editor, &ctx, Key::C, egui::Modifiers::NONE);
    drag(&mut editor, &ctx, start, end);
    let cropped = editor.shared.lock().unwrap().doc.clone();
    assert_ne!(cropped.objects[0].options.crop, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(cropped.objects[0].content, before.objects[0].content);
    assert_eq!(cropped.version, 3);
    cropped.validate().unwrap();
    key(&mut editor, &ctx, Key::Z, egui::Modifiers::CTRL);
    assert_eq!(editor.shared.lock().unwrap().doc.objects, before.objects);
    let camera = editor.shared.lock().unwrap().doc.camera;
    key(&mut editor, &ctx, Key::Z, egui::Modifiers::NONE);
    assert!(editor.focus_camera.is_some());
    key(&mut editor, &ctx, Key::Z, egui::Modifiers::NONE);
    assert_eq!(editor.shared.lock().unwrap().doc.camera, camera);
    drop(editor);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn canvas_lock_blocks_import_and_isolated_objects_remain_selectable_after_deselect() {
    let (root, mut editor, ctx) = fixture("reference-lock");
    editor.shared.lock().unwrap().doc.canvas.locked = true;
    editor.add_files(&[root.join("another.png")], None);
    assert_eq!(editor.shared.lock().unwrap().doc.objects.len(), 2);
    editor.isolate = true;
    editor.isolated_ids = HashSet::from([1]);
    editor.selected.clear();
    let doc = editor.shared.lock().unwrap().doc.clone();
    assert_eq!(editor.hit(&doc, doc.objects[0].center(), false), Some(1));
    assert_eq!(editor.hit(&doc, doc.objects[1].center(), false), None);
    frame(&mut editor, &ctx, vec![]);
    drop(editor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn right_click_selects_clicked_image_and_menu_requests_its_source_folder() {
    let (root, mut editor, ctx) = fixture("reference-context");
    editor.selected = HashSet::from([1]);
    let doc = editor.shared.lock().unwrap().doc.clone();
    let p = doc.camera.screen(doc.objects[1].center(), editor.area);
    for pressed in [true, false] {
        frame(
            &mut editor,
            &ctx,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::PointerButton {
                    pos: p,
                    button: PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(editor.context_target, Some(2));
    assert_eq!(editor.selected, HashSet::from([2]));
    let output = frame(&mut editor, &ctx, vec![]);
    let target = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "Open source folder" => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("Open source folder is visible in the right-click menu");
    frame(
        &mut editor,
        &ctx,
        vec![egui::Event::PointerMoved(target), button(target, true)],
    );
    frame(&mut editor, &ctx, vec![button(target, false)]);
    let request = ctx
        .data(|d| d.get_temp::<RevealSource>(egui::Id::new("board_reveal_source")))
        .expect("menu action queues navigation");
    assert_eq!(
        request.source,
        ImageSrc::File(resolve(Path::new("missing-2.png"), &editor.path()))
    );
    assert_eq!(request.board, editor.path());
    drop(editor);
    std::fs::remove_dir_all(root).unwrap();
}
