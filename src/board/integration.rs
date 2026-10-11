use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reveal_original_preserves_canvas_and_selects_local_or_archive_source() {
        use std::io::Write;
        let root = super::super::tests::fixture("board-reveal");
        let folder = root.join("source");
        std::fs::create_dir(&folder).unwrap();
        let original = folder.join("image.png");
        image::RgbImage::from_pixel(24, 36, image::Rgb([70, 130, 190]))
            .save(&original)
            .unwrap();
        let archive = folder.join("images.cbz");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        writer
            .start_file(
                "chapter/image.png",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer
            .write_all(&std::fs::read(&original).unwrap())
            .unwrap();
        writer.finish().unwrap();
        let board = root.join("board.vcboard");
        write(&board, &Document::new("Kept".into())).unwrap();
        for source in [
            ImageSrc::File(original.clone()),
            ImageSrc::Entry {
                archive: archive.clone(),
                name: "chapter/image.png".into(),
            },
        ] {
            for dual in [false, true] {
                let mut app = App::new(None);
                let ctx = egui::Context::default();
                app.select(Node::Dir(root.clone()), &ctx);
                app.set_dual(dual, &ctx);
                if dual {
                    app.switch_pane(1);
                    app.select(Node::Dir(root.clone()), &ctx);
                }
                let inactive = app.inactive.as_ref().and_then(|a| a.selected.clone());
                app.tabs[app.active_tab].locked = true;
                app.board = Some(Editor::open(&board).unwrap());
                let board_path = app.board.as_ref().unwrap().path();
                app.files.search.query = "hide everything".into();
                app.files.search.kind = 6;
                ctx.data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("board_reveal_source"),
                        RevealSource {
                            source: source.clone(),
                            board: board_path.clone(),
                        },
                    )
                });
                app.process_board_source_request(&ctx);
                assert_eq!(app.canvas.editor.as_ref().unwrap().path(), board_path);
                assert_eq!(app.images[app.current.unwrap()].src, source);
                assert_eq!(app.files.selected.len(), 1);
                assert_eq!(app.files.reveal_row, app.files.anchor);
                assert!(!app.files.search.results && app.files.search.query.is_empty());
                assert_eq!(
                    app.inactive.as_ref().and_then(|a| a.selected.clone()),
                    inactive
                );
                assert_eq!(app.active_pane, usize::from(dual));
                drop(app);
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl App {
    /// Handle after the previous frame has restored the active file pane.
    pub(crate) fn process_board_source_request(&mut self, ctx: &egui::Context) {
        let Some(request) = ctx.data_mut(|d| {
            let key = egui::Id::new("board_reveal_source");
            let request = d.get_temp::<RevealSource>(key);
            d.remove::<RevealSource>(key);
            request
        }) else {
            return;
        };
        let path = match &request.source {
            ImageSrc::File(path) => path,
            ImageSrc::Entry { archive, .. } => archive,
        };
        if !path.is_file() {
            self.file_message = format!("Source file is unavailable: {}", path.display());
            return;
        }
        // Keep standalone boards visible too; collection canvases are already pinned.
        if self.canvas.editor.is_none() {
            if self
                .board
                .as_ref()
                .is_some_and(|e| e.path() == request.board)
            {
                self.canvas.editor = self.board.take();
            } else if let Some(other) = &mut self.inactive {
                if other
                    .board
                    .as_ref()
                    .is_some_and(|e| e.path() == request.board)
                {
                    self.canvas.editor = other.board.take();
                }
            }
        }
        if self.canvas.fullscreen {
            self.canvas.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if self.fullscreen {
            self.set_fullscreen(ctx, false);
        }
        self.settings.review = false;
        self.settings.show_preview = true;
        self.files.search.query.clear();
        self.files.search.kind = 0;
        match &request.source {
            ImageSrc::File(path) => self.reveal_original(path, ctx),
            ImageSrc::Entry { archive, name } => {
                let node = match name.rsplit_once('/') {
                    Some((parent, _)) => Node::ArchiveDir {
                        archive: archive.clone(),
                        prefix: format!("{parent}/"),
                    },
                    None => Node::Archive(archive.clone()),
                };
                self.select(node, ctx);
                if let Some(index) = self.images.iter().position(|im| im.src == request.source) {
                    self.goto(index, ctx);
                } else {
                    self.file_message = "The source image is unavailable inside the archive".into();
                }
            }
        }
        self.files.reveal_row = self.files.anchor;
    }
    pub(crate) fn board_focus(&self) -> bool {
        self.canvas
            .editor
            .as_ref()
            .is_some_and(Editor::owns_keyboard)
            || self.board.as_ref().is_some_and(Editor::owns_keyboard)
    }
    pub(crate) fn close_board(&mut self) -> bool {
        if let Some(board) = &self.board {
            if let Err(error) = board.flush() {
                self.file_message = error.to_string();
                return false;
            }
        }
        self.board = None;
        true
    }
    pub(crate) fn create_board(&mut self, sources: Vec<ImageSrc>, ctx: &egui::Context) {
        self.files.new_board_sources = sources;
        self.files.collection_dialog = Some((None, String::new(), Vec::new()));
        ctx.request_repaint();
    }
    pub(crate) fn open_collection_board(&mut self, id: &str, ctx: &egui::Context) {
        if self.canvas.id.as_deref() == Some(id) {
            return;
        }
        if !self.clear_collection_canvas() {
            return;
        }
        match collections::storage::board_path(id).and_then(|p| Editor::open(&p)) {
            Ok(mut editor) => {
                editor.set_collection(true);
                self.canvas.editor = Some(editor);
                self.canvas.id = Some(id.into());
                self.settings.show_preview = true;
                ctx.request_repaint();
            }
            Err(e) => self.file_message = e.to_string(),
        }
    }
    pub(crate) fn board_dropped_files(&mut self, paths: &[PathBuf], ctx: &egui::Context) -> bool {
        if paths.is_empty() {
            return false;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        if let Some(board) = &mut self.canvas.editor {
            if pointer.is_some_and(|p| board.area.contains(p)) {
                board.add_files(paths, pointer);
                return true;
            }
        }
        if let Some(board) = &mut self.board {
            if pointer.is_some_and(|p| board.area.contains(p)) {
                board.add_files(paths, pointer);
                return true;
            }
        }
        if let Some(other) = self.inactive.as_mut() {
            if let Some(board) = &mut other.board {
                if pointer.is_some_and(|p| board.area.contains(p)) {
                    board.add_files(paths, pointer);
                    return true;
                }
            }
        }
        false
    }
    pub(crate) fn tick_boards(&mut self, ctx: &egui::Context) {
        if let Some(board) = &mut self.canvas.editor {
            board.update_focus(ctx);
            board.tick(ctx);
        }
        if let Some(board) = &mut self.board {
            board.update_focus(ctx);
            board.tick(ctx);
        }
        if let Some(other) = self.inactive.as_mut() {
            other.tick_boards(ctx);
        }
        for tab in &mut self.tabs {
            if let Some(state) = tab.state.as_mut() {
                state.tick_boards(ctx);
            }
        }
    }
}
