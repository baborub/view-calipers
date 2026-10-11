use super::*;
fn fixture(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "vc-collection-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    }
}
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::SidePanel::left("tree")
                .exact_width(220.0)
                .show(ctx, |ui| {
                    app.collection_sidebar(ui);
                });
            egui::SidePanel::right("preview")
                .exact_width(600.0)
                .show(ctx, |ui| app.show_preview_panel(ui));
            egui::CentralPanel::default().show(ctx, |ui| app.show_panes(ui));
        },
    )
}
#[test]
fn canvas_selection_is_pinned_below_active_file_and_deselects_only_in_sidebar() {
    let _library = test_guard();
    let root = fixture("pinned");
    let pic = root.join("source.png");
    image::RgbImage::new(30, 40).save(&pic).unwrap();
    let id = create("Pinned canvas", &[pic.clone()]).unwrap();
    let path = storage::board_path(&id).unwrap();
    assert_eq!(path.file_name().unwrap(), "Pinned canvas.vcboard");
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "Pinned canvas");
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.select(Node::Dir(root.clone()), &ctx);
    let out = frame(&mut app, &ctx, vec![]);
    let pos = out
        .shapes
        .iter()
        .find_map(|s| {
            if let egui::Shape::Text(t) = &s.shape {
                (t.galley.job.text == "Pinned canvas").then(|| t.pos + Vec2::splat(5.0))
            } else {
                None
            }
        })
        .unwrap();
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(pos), button(pos, true)],
    );
    frame(&mut app, &ctx, vec![button(pos, false)]);
    assert_eq!(app.selected, Some(Node::Dir(root.clone())));
    assert!(!app.dual_panes);
    assert_eq!(app.canvas.id.as_deref(), Some(id.as_str()));
    assert!(app.canvas.editor.as_ref().unwrap().area.top() > 450.0);
    let empty = app.canvas.editor.as_ref().unwrap().area.right_bottom() - Vec2::splat(20.0);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(empty), button(empty, true)],
    );
    frame(&mut app, &ctx, vec![button(empty, false)]);
    assert_eq!(app.canvas.id.as_deref(), Some(id.as_str()));
    app.new_tab(false, &ctx);
    app.switch_tab(0);
    app.set_dual(true, &ctx);
    app.switch_pane(1);
    app.select(Node::Dir(root.clone()), &ctx);
    app.goto(0, &ctx);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.canvas.id.as_deref(), Some(id.as_str()));
    assert_eq!(app.current, Some(0));
    app.set_dual(false, &ctx);
    frame(&mut app, &ctx, vec![]);
    assert!(app.canvas.editor.is_some());
    let folder = path.parent().unwrap();
    app.select(Node::Dir(folder.into()), &ctx);
    assert!(app.files.rows.iter().all(|r| r.extension() != "vcboard"));
    frame(&mut app, &ctx, vec![]);
    let blank = app.canvas.blank.unwrap().center();
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(blank), button(blank, true)],
    );
    frame(&mut app, &ctx, vec![button(blank, false)]);
    assert!(app.canvas.id.is_none() && app.canvas.editor.is_none());
    assert!(pic.exists());
    update(|l| {
        l.collections.retain(|c| c.id != id);
        Ok(())
    })
    .unwrap();
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn import_survives_deselection_and_rename_and_library_relocation_preserve_owned_images() {
    let _library = test_guard();
    let id = create("Imported images", &[]).unwrap();
    let path = storage::board_path(&id).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        32,
        48,
        image::Rgb([180, 50, 120]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.open_collection_board(&id, &ctx);
    app.canvas.editor.as_mut().unwrap().import_images(
        crate::external_images::Input::Bytes {
            name: "from-browser.png".into(),
            bytes: bytes.into_inner(),
        },
        None,
        &ctx,
    );
    app.clear_collection_canvas();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while board::imports_running() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let doc = board::read(&path).unwrap();
    assert_eq!(doc.objects.len(), 1);
    let board::Content::Image { source } = &doc.objects[0].content else {
        panic!()
    };
    assert!(source.is_relative());
    assert!(path.parent().unwrap().join(source).is_file());
    storage::rename(&id, "Renamed canvas").unwrap();
    let renamed = storage::board_path(&id).unwrap();
    assert!(!path.exists());
    let doc = board::read(&renamed).unwrap();
    assert_eq!(doc.title, "Renamed canvas");
    assert_eq!(doc.objects.len(), 1);
    assert!(renamed.parent().unwrap().join(source).is_file());
    let old = storage::root();
    let moved = fixture("relocated");
    storage::relocate(&moved).unwrap();
    let relocated = storage::board_path(&id).unwrap();
    assert!(relocated.parent().unwrap().join(source).is_file());
    assert!(renamed.is_file());
    assert!(storage::relocate(&moved.join("child")).is_err());
    assert!(storage::rename(&id, "../escape").is_err());
    {
        let mut s = store().lock().unwrap();
        s.root = old;
    }
    update(|l| {
        l.collections.retain(|c| c.id != id);
        Ok(())
    })
    .unwrap();
    std::fs::remove_dir_all(moved).unwrap();
}
#[test]
fn legacy_layout_notes_ink_and_relative_sources_migrate_without_modifying_old_data() {
    let root = fixture("migration");
    let old = root.join("old");
    std::fs::create_dir(&old).unwrap();
    let source = old.join("image.png");
    image::RgbImage::new(8, 8).save(&source).unwrap();
    let path = old.join("board.vcboard");
    let mut doc = board::Document::new("Before".into());
    doc.add_images(&[source.clone()], &path, Pos2::new(80.0, 125.0));
    doc.objects[0].angle = 0.75;
    doc.objects.push(board::Object {
        id: 2,
        center: [12.0, 15.0],
        size: [100.0, 80.0],
        angle: 0.0,
        options: Default::default(),
        content: board::Content::Text {
            text: "Keep this note".into(),
            color: [255; 4],
        },
    });
    doc.objects.push(board::Object {
        id: 3,
        center: [0.0, 0.0],
        size: [100.0, 80.0],
        angle: 0.0,
        options: Default::default(),
        content: board::Content::Ink {
            points: vec![[0.0, 0.0], [0.3, 0.5]],
            color: [255; 4],
            width: 3.0,
        },
    });
    board::write(&path, &doc).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut library = Library::default();
    library.collections.push(Collection {
        id: "legacy".into(),
        name: "Legacy / Board".into(),
        paths: vec![],
        folder: String::new(),
    });
    let target = root.join("new");
    storage::migrate(&target, &mut library, |_| path.clone()).unwrap();
    let c = &library.collections[0];
    let migrated = target.join(&c.folder).join(format!("{}.vcboard", c.folder));
    let loaded = board::read(&migrated).unwrap();
    assert_eq!(loaded.objects.len(), 3);
    assert_eq!(loaded.objects[0].angle, 0.75);
    assert_eq!(loaded.objects[1], doc.objects[1]);
    assert_eq!(loaded.objects[2], doc.objects[2]);
    let board::Content::Image { source: reference } = &loaded.objects[0].content else {
        panic!()
    };
    assert_eq!(
        std::fs::canonicalize(board::resolve(reference, &migrated)).unwrap(),
        std::fs::canonicalize(source).unwrap()
    );
    assert_eq!(before, std::fs::read(&path).unwrap());
    storage::migrate(&target, &mut library, |_| path.clone()).unwrap();
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn drag_files_and_archive_images_from_both_panes_into_pinned_canvas() {
    let _library = test_guard();
    use std::io::Write;
    let root = fixture("pinned-drag");
    let images = root.join("images");
    std::fs::create_dir(&images).unwrap();
    for name in ["01.png", "02.png"] {
        image::RgbImage::new(40, 60)
            .save(images.join(name))
            .unwrap();
    }
    let archive = root.join("images.cbz");
    let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
    for name in ["01.png", "02.png"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&std::fs::read(images.join(name)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap();
    for source_pane in [0, 1] {
        for archived in [false, true] {
            let ctx = egui::Context::default();
            let mut app = App::new(None);
            let id = create(&format!("Pinned drag {source_pane} {archived}"), &[]).unwrap();
            app.open_collection_board(&id, &ctx);
            app.set_dual(true, &ctx);
            app.switch_pane(source_pane);
            app.select(
                if archived {
                    Node::ArchiveDir {
                        archive: archive.clone(),
                        prefix: String::new(),
                    }
                } else {
                    Node::Dir(images.clone())
                },
                &ctx,
            );
            app.files.selected = HashSet::from([0, 1]);
            frame(&mut app, &ctx, vec![]);
            frame(&mut app, &ctx, vec![]);
            let start = app.files.item_rects[0].center();
            let target = app.canvas.editor.as_ref().unwrap().area.center();
            frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(start + Vec2::splat(15.0))],
            );
            assert!(
                egui::DragAndDrop::has_any_payload(&ctx),
                "source pane {source_pane}, archived={archived}"
            );
            frame(&mut app, &ctx, vec![egui::Event::PointerMoved(target)]);
            frame(&mut app, &ctx, vec![button(target, false)]);
            assert!(app.files.pending.is_none());
            let path = storage::board_path(&id).unwrap();
            app.canvas.editor.as_ref().unwrap().flush().unwrap();
            assert_eq!(
                board::read(&path).unwrap().objects.len(),
                2,
                "source pane {source_pane}, archived={archived}"
            );
            drop(app);
            update(|lib| {
                lib.collections.retain(|c| c.id != id);
                Ok(())
            })
            .unwrap();
        }
    }
    assert!(
        images.join("01.png").is_file() && images.join("02.png").is_file() && archive.is_file()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn video_and_markdown_drag_from_either_pane_and_create_from_selection() {
    let _library = test_guard();
    let root = fixture("media-drag");
    let movie = root.join("movie.mp4");
    let notes = root.join("notes.markdown");
    let out = video::command("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=90x160:r=10",
            "-t",
            "0.5",
            "-c:v",
            "mpeg4",
        ])
        .arg(&movie)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::write(&notes, "# Reference notes\n\nA **formatted** card.").unwrap();
    for pane in [0, 1] {
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        let id = create(&format!("Media drag {pane}"), &[]).unwrap();
        app.open_collection_board(&id, &ctx);
        app.set_dual(true, &ctx);
        app.switch_pane(pane);
        app.select(Node::Dir(root.clone()), &ctx);
        app.files.selected = HashSet::from([0, 1]);
        assert_eq!(app.selected_board_sources().len(), 2);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let start = app.files.item_rects[0].center();
        let target = app.canvas.editor.as_ref().unwrap().area.center();
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(start + Vec2::splat(15.0))],
        );
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(target)]);
        frame(&mut app, &ctx, vec![button(target, false)]);
        assert!(app.files.pending.is_none());
        app.canvas.editor.as_ref().unwrap().flush().unwrap();
        let doc = board::read(&storage::board_path(&id).unwrap()).unwrap();
        assert_eq!(doc.objects.len(), 2);
        assert!(matches!(
            doc.objects[0].content,
            board::Content::Video { .. }
        ));
        assert!(matches!(
            doc.objects[1].content,
            board::Content::Markdown { .. }
        ));
        drop(app);
        update(|lib| {
            lib.collections.retain(|c| c.id != id);
            Ok(())
        })
        .unwrap();
    }
    assert!(movie.is_file() && notes.is_file());
    // Cancelling a canvas does not block the UI on its in-flight poster probe.
    // On Windows, allow that FFmpeg process to release the synthetic video
    // before removing the fixture; unexpected errors still fail immediately.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => break,
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => panic!("Cannot remove media fixture: {error}"),
        }
    }
}
