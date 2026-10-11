//! Named, explicit snapshots. Store references and UI state, never media bytes.
use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct State {
    manage: bool,
    dialog: Option<(Option<String>, String)>,
    delete: Option<String>,
    restore: Option<String>,
    message: String,
}
#[derive(Clone)]
struct Tab {
    node: Node,
    locked: bool,
    view: folder_views::View,
    custom: bool,
    current: Option<String>,
    selection: Vec<PathBuf>,
    expanded: Vec<Node>,
    query: String,
    kind: usize,
    regex: bool,
}
#[derive(Clone)]
struct Pane {
    active: usize,
    tabs: Vec<Tab>,
}
struct Snapshot {
    settings: settings::Settings,
    active: usize,
    panes: Vec<Pane>,
}

fn hex(s: &str) -> String {
    s.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Result<String> {
    anyhow::ensure!(s.len() % 2 == 0 && s.is_ascii(), "Invalid workspace text");
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(String::from_utf8(bytes)?)
}
fn node_text(n: &Node) -> String {
    match n {
        Node::Computer => "pc".into(),
        Node::Dir(p) => format!("dir\n{}", p.display()),
        Node::Archive(p) => format!("archive\n{}", p.display()),
        Node::ArchiveDir { archive, prefix } => format!("entry\n{}\n{prefix}", archive.display()),
        Node::Collection { id, name } => format!("collection\n{id}\n{name}"),
    }
}
fn node_parse(s: &str) -> Result<Node> {
    let a: Vec<_> = s.splitn(3, '\n').collect();
    Ok(match a.as_slice() {
        ["pc"] => Node::Computer,
        ["dir", p] if Path::new(p).is_absolute() => Node::Dir(p.into()),
        ["archive", p] if Path::new(p).is_absolute() => Node::Archive(p.into()),
        ["entry", p, prefix] if Path::new(p).is_absolute() => Node::ArchiveDir {
            archive: p.into(),
            prefix: prefix.to_string(),
        },
        ["collection", id, name] => Node::Collection {
            id: id.to_string(),
            name: name.to_string(),
        },
        _ => anyhow::bail!("Invalid workspace location"),
    })
}
fn source_text(s: &ImageSrc) -> String {
    match s {
        ImageSrc::File(p) => format!("file\n{}", p.display()),
        ImageSrc::Entry { archive, name } => format!("entry\n{}\n{name}", archive.display()),
    }
}
fn capture_tab(app: &App, locked: bool) -> Tab {
    Tab {
        node: app.selected.clone().unwrap_or(Node::Computer),
        locked,
        view: folder_views::View {
            details: app.files.details,
            sort: app.files.sort,
            thumb: app.thumb_size,
            layout: app.files.columns.clone(),
        },
        custom: app.files.collection_custom,
        current: app
            .current
            .and_then(|i| app.images.get(i))
            .map(|i| source_text(&i.src)),
        selection: app
            .files
            .selected
            .iter()
            .filter_map(|i| app.files.rows.get(*i)?.path.clone())
            .collect(),
        expanded: app.expanded.iter().cloned().collect(),
        query: if app.files.search.scope == 0 && !app.files.search.results {
            app.files.search.query.clone()
        } else {
            String::new()
        },
        kind: if app.files.search.scope == 0 {
            app.files.search.kind
        } else {
            0
        },
        regex: app.files.search.regex,
    }
}
fn capture_pane(app: &App) -> Pane {
    Pane {
        active: app.active_tab,
        tabs: app
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                capture_tab(
                    if i == app.active_tab {
                        app
                    } else {
                        t.state.as_ref().expect("Inactive tab state")
                    },
                    t.locked,
                )
            })
            .collect(),
    }
}
impl Snapshot {
    fn capture(app: &App) -> Self {
        let mut settings = app.settings.clone();
        settings.dual = app.dual_panes;
        settings.split_preview = app.split_preview;
        settings.pane_fraction = app.pane_fraction;
        settings.preview_fraction = app.preview_fraction;
        let mut panes = vec![capture_pane(app)];
        if app.dual_panes {
            if let Some(other) = app.inactive.as_ref() {
                panes.push(capture_pane(other));
            }
        }
        if app.active_pane == 1 && panes.len() == 2 {
            panes.swap(0, 1);
        }
        Self {
            settings,
            active: app.active_pane,
            panes,
        }
    }
    fn text(&self) -> String {
        let mut text = format!(
            "view-calipers workspace v1\nL\t{}\nA\t{}\n",
            hex(&self.settings.text()),
            self.active
        );
        for (p, pane) in self.panes.iter().enumerate() {
            text.push_str(&format!("P\t{p}\t{}\n", pane.active));
            for t in &pane.tabs {
                let mut views = BTreeMap::new();
                views.insert("view".into(), t.view.clone());
                let paths = t
                    .selection
                    .iter()
                    .map(|p| hex(&p.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(",");
                let expanded = t
                    .expanded
                    .iter()
                    .map(|n| hex(&node_text(n)))
                    .collect::<Vec<_>>()
                    .join(",");
                text.push_str(&format!(
                    "T\t{p}\t{}\t{}\t{}\t{}\t{}\t{paths}\t{expanded}\t{}\t{}\t{}\n",
                    t.locked,
                    hex(&node_text(&t.node)),
                    hex(&folder_views::encode(&views)),
                    t.custom,
                    hex(t.current.as_deref().unwrap_or("")),
                    hex(&t.query),
                    t.kind,
                    t.regex
                ));
            }
        }
        text
    }
    fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        anyhow::ensure!(
            lines.next() == Some("view-calipers workspace v1"),
            "Unsupported workspace format"
        );
        let mut settings = None;
        let mut active = None;
        let mut panes: Vec<Pane> = vec![];
        for line in lines {
            let a: Vec<_> = line.split('\t').collect();
            match a.as_slice() {
                ["L", v] if settings.is_none() => {
                    settings = Some(settings::Settings::parse(&unhex(v)?))
                }
                ["A", v] if active.is_none() => active = Some(v.parse::<usize>()?),
                ["P", p, index] => {
                    anyhow::ensure!(
                        p.parse::<usize>()? == panes.len() && panes.len() < 2,
                        "Invalid workspace pane"
                    );
                    panes.push(Pane {
                        active: index.parse()?,
                        tabs: vec![],
                    });
                }
                ["T", p, locked, node, view, custom, current, paths, expanded, query, kind, regex] =>
                {
                    let pane = panes
                        .get_mut(p.parse::<usize>()?)
                        .ok_or_else(|| anyhow!("Invalid workspace tab"))?;
                    anyhow::ensure!(pane.tabs.len() < 128, "Too many workspace tabs");
                    let view = folder_views::parse(&unhex(view)?)
                        .remove("view")
                        .ok_or_else(|| anyhow!("Invalid workspace view"))?;
                    let current = unhex(current)?;
                    let kind = kind.parse::<usize>()?;
                    anyhow::ensure!(kind < 7, "Invalid filter type");
                    let selection = paths
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| unhex(s).map(PathBuf::from))
                        .collect::<Result<Vec<_>>>()?;
                    anyhow::ensure!(
                        selection.iter().all(|p| p.is_absolute()),
                        "Invalid selected path"
                    );
                    let expanded = expanded
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| node_parse(&unhex(s)?))
                        .collect::<Result<Vec<_>>>()?;
                    pane.tabs.push(Tab {
                        node: node_parse(&unhex(node)?)?,
                        locked: locked.parse()?,
                        view,
                        custom: custom.parse()?,
                        current: (!current.is_empty()).then_some(current),
                        selection,
                        expanded,
                        query: unhex(query)?,
                        kind,
                        regex: regex.parse()?,
                    });
                }
                _ => anyhow::bail!("Invalid workspace record"),
            }
        }
        let settings = settings.ok_or_else(|| anyhow!("Missing workspace layout"))?;
        let active = active.ok_or_else(|| anyhow!("Missing active pane"))?;
        anyhow::ensure!(
            !panes.is_empty()
                && active < panes.len()
                && settings.dual == (panes.len() == 2)
                && (!settings.split_preview || settings.dual),
            "Invalid workspace layout"
        );
        anyhow::ensure!(
            panes
                .iter()
                .all(|p| !p.tabs.is_empty() && p.active < p.tabs.len()),
            "Invalid active tab"
        );
        Ok(Self {
            settings,
            active,
            panes,
        })
    }
}
fn directory() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!(
            "view-calipers-workspaces-test-{}",
            std::process::id()
        ))
    } else {
        settings::Settings::path().with_file_name("workspaces")
    }
}
fn valid_name(name: &str) -> Result<&str> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty()
            && name.chars().count() <= 80
            && name.len() <= 120
            && !name.chars().any(char::is_control),
        "Name is empty, contains a control character, or is too long"
    );
    Ok(name)
}
fn path(name: &str) -> Result<PathBuf> {
    Ok(directory().join(format!("{}.workspace", hex(valid_name(name)?))))
}
fn names() -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(directory()) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut names = vec![];
    for entry in entries {
        let p = entry?.path();
        if p.extension().is_some_and(|s| s == "workspace") {
            if let Some(s) = p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| unhex(s).ok())
            {
                names.push(s);
            }
        }
    }
    names.sort_by(|a, b| natural_cmp(a, b));
    Ok(names)
}
fn write(name: &str, snapshot: &Snapshot, replace: bool) -> Result<()> {
    let p = path(name)?;
    std::fs::create_dir_all(directory())?;
    anyhow::ensure!(
        replace || !p.exists(),
        "That name already exists. Use Update in Manage workspaces."
    );
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, snapshot.text())?;
    std::fs::rename(tmp, p)?;
    Ok(())
}
fn available(node: &Node) -> bool {
    match node {
        Node::Computer => true,
        Node::Dir(p) => p.is_dir(),
        Node::Archive(p) | Node::ArchiveDir { archive: p, .. } => p.is_file(),
        Node::Collection { id, .. } => collections::get(id).is_some(),
    }
}
impl App {
    pub(super) fn workspace_capture(&mut self, ctx: &egui::Context) {
        if std::env::var_os("ARCHVIEW_UI_CAPTURE").is_none() {
            return;
        }
        let mode = std::env::var("ARCHVIEW_UI_CHECK").unwrap_or_default();
        if mode.contains("workspace-save") {
            self.pane_fraction = 0.4;
            self.preview_fraction = 0.6;
            let result = write("Review workspace", &Snapshot::capture(self), true);
            self.workspace_result(result, "Workspace saved");
            self.workspaces.manage = true;
        } else if mode.contains("workspace-restore") {
            let result = (|| {
                let text = std::fs::read_to_string(path("Review workspace")?)?;
                self.restore_workspace(Snapshot::parse(&text)?, ctx)
            })();
            if let Err(error) = result {
                self.workspace_result(Err(error), "");
            }
            self.workspaces.manage = true;
        }
    }
    pub(super) fn workspace_modal(&self) -> bool {
        self.workspaces.manage
            || self.workspaces.dialog.is_some()
            || self.workspaces.delete.is_some()
    }
    pub(super) fn workspaces_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Workspaces", |ui| {
            if ui.button("Save workspace as…").clicked() {
                self.workspaces.dialog = Some((None, String::new()));
                ui.close_menu();
            }
            if ui.button("Manage workspaces…").clicked() {
                self.workspaces.manage = true;
                ui.close_menu();
            }
            ui.separator();
            match names() {
                Ok(names) => {
                    if names.is_empty() {
                        ui.label("No saved workspaces");
                    }
                    for name in names {
                        if ui.button(&name).clicked() {
                            self.workspaces.restore = Some(name);
                            ui.close_menu();
                        }
                    }
                }
                Err(e) => {
                    ui.label(format!("Workspaces: {e}"));
                }
            }
        });
    }
    fn workspace_result(&mut self, result: Result<()>, message: &str) {
        self.workspaces.message = match result {
            Ok(()) => message.into(),
            Err(e) => format!("Workspaces: {e}"),
        };
        self.file_message = self.workspaces.message.clone();
    }
    fn retire_workspace(&mut self) {
        self.save_folder_view(true);
        self.photos.cancel();
        self.files.search.client.cancel();
        self.thumb_generation.fetch_add(1, AtomicOrdering::Relaxed);
        if let Some(v) = &mut self.video {
            v.pause();
        }
        self.retired_renderers.push(self.model_renderer.clone());
        for t in &mut self.tabs {
            if let Some(state) = &mut t.state {
                state.retire_workspace();
                self.retired_renderers.append(&mut state.retired_renderers);
            }
        }
    }
    fn restore_workspace(&mut self, snapshot: Snapshot, ctx: &egui::Context) -> Result<()> {
        anyhow::ensure!(
            self.file_job.is_none(),
            "Wait for the current file operation to finish"
        );
        let mut missing = 0;
        fn build(
            pane: &Pane,
            settings: &settings::Settings,
            ctx: &egui::Context,
            missing: &mut usize,
        ) -> App {
            let mut states = vec![];
            for t in &pane.tabs {
                let mut app = App::new(None);
                app.settings = settings.clone();
                app.settings_initialized = true;
                app.cache_thumbnails = settings.cache;
                app.thumb_size = t.view.thumb;
                if available(&t.node) {
                    app.select(t.node.clone(), ctx);
                } else {
                    *missing += 1;
                    app.select(Node::Computer, ctx);
                }
                app.expanded = t
                    .expanded
                    .iter()
                    .filter(|n| available(n))
                    .cloned()
                    .collect();
                app.reveal_ancestors(&app.selected.clone().unwrap_or(Node::Computer));
                app.files.details = t.view.details;
                app.files.sort = t.view.sort;
                app.files.columns = t.view.layout.clone();
                app.thumb_size = t.view.thumb;
                app.files.collection_custom = t.custom;
                app.files.search.query = t.query.clone();
                app.files.search.kind = t.kind;
                app.files.search.regex = t.regex;
                app.apply_file_filter();
                app.files.selected = app
                    .files
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(i, r)| {
                        r.path
                            .as_ref()
                            .filter(|p| t.selection.contains(p))
                            .map(|_| i)
                    })
                    .collect();
                app.files.anchor = app.files.selected.iter().copied().min();
                if let Some(current) = &t.current {
                    if let Some(i) = app
                        .images
                        .iter()
                        .position(|i| source_text(&i.src) == *current)
                    {
                        app.goto(i, ctx);
                    }
                }
                states.push(app);
            }
            let titles: Vec<_> = states
                .iter()
                .map(|state| {
                    state
                        .selected
                        .as_ref()
                        .map(Node::label)
                        .unwrap_or_else(|| "This PC".into())
                })
                .collect();
            let mut active = states.remove(pane.active);
            let mut iter = states.into_iter();
            active.tabs = pane
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| BrowserTab {
                    locked: t.locked,
                    title: titles[i].clone(),
                    state: if i == pane.active {
                        None
                    } else {
                        Some(Box::new(iter.next().unwrap()))
                    },
                })
                .collect();
            active.active_tab = pane.active;
            active
        }
        let mut panes = snapshot
            .panes
            .iter()
            .map(|p| build(p, &self.settings, ctx, &mut missing))
            .collect::<Vec<_>>();
        self.retire_workspace();
        if let Some(mut other) = self.inactive.take() {
            other.retire_workspace();
            self.retired_renderers.append(&mut other.retired_renderers);
        }
        let mut active = panes.remove(snapshot.active);
        self.swap_browser(&mut active);
        self.tabs = active.tabs;
        self.active_tab = active.active_tab;
        self.inactive = panes.pop().map(Box::new);
        self.active_pane = snapshot.active;
        self.dual_panes = snapshot.settings.dual;
        macro_rules! layout {($($f:ident),*)=>{$(self.settings.$f=snapshot.settings.$f;)*};}
        layout!(
            dual,
            split_preview,
            compare_fullscreen,
            show_tree,
            show_preview,
            review,
            comfortable,
            tree_width,
            preview_width,
            pane_fraction,
            preview_fraction,
            fullscreen_fraction
        );
        self.split_preview = self.settings.split_preview;
        self.pane_fraction = self.settings.pane_fraction;
        self.preview_fraction = self.settings.preview_fraction;
        self.fullscreen = false;
        self.workbench.paired_fullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.data_mut(|d| {
            d.remove::<egui::containers::panel::PanelState>(egui::Id::new("tree"));
            d.remove::<egui::containers::panel::PanelState>(egui::Id::new("preview"));
        });
        ui_theme::apply(ctx, self.settings.comfortable);
        #[cfg(not(test))]
        self.save_preferences();
        self.workspaces.message = if missing == 0 {
            "Workspace restored".into()
        } else {
            format!("Workspace restored · {missing} unavailable location(s) opened as This PC")
        };
        self.file_message = self.workspaces.message.clone();
        ctx.request_repaint();
        Ok(())
    }
    pub(super) fn workspace_window(&mut self, ctx: &egui::Context) {
        let mut open = self.workspaces.manage;
        if open {
            egui::Window::new("Manage workspaces")
                .open(&mut open)
                .default_width(460.0)
                .show(ctx, |ui| {
                    ui.label("Update replaces a saved workspace with the current tabs and layout.");
                    match names() {
                        Ok(names) => {
                            egui::ScrollArea::vertical()
                                .max_height(450.0)
                                .show(ui, |ui| {
                                    for name in names {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [180.0, ui.spacing().interact_size.y],
                                                egui::Label::new(&name).truncate(),
                                            )
                                            .on_hover_text(&name);
                                            if ui.button("Restore").clicked() {
                                                self.workspaces.restore = Some(name.clone());
                                            }
                                            if ui.button("Update").clicked() {
                                                let result =
                                                    write(&name, &Snapshot::capture(self), true);
                                                self.workspace_result(result, "Workspace updated");
                                            }
                                            if ui.button("Rename").clicked() {
                                                self.workspaces.dialog =
                                                    Some((Some(name.clone()), name.clone()));
                                            }
                                            if ui.button("Delete").clicked() {
                                                self.workspaces.delete = Some(name.clone());
                                            }
                                        });
                                    }
                                });
                        }
                        Err(e) => {
                            ui.label(format!("Workspaces: {e}"));
                        }
                    }
                    ui.label(&self.workspaces.message);
                });
            self.workspaces.manage = open;
        }
        if let Some((original, mut name)) = self.workspaces.dialog.clone() {
            let mut action = 0;
            egui::Window::new(if original.is_some() {
                "Rename workspace"
            } else {
                "Save workspace"
            })
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut name);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        action = 1;
                    }
                    if ui.button("Cancel").clicked() {
                        action = 2;
                    }
                });
                ui.label(&self.workspaces.message);
            });
            self.workspaces.dialog = Some((original.clone(), name.clone()));
            if action == 2 {
                self.workspaces.dialog = None;
            } else if action == 1 {
                let result = (|| {
                    valid_name(&name)?;
                    if let Some(old) = original {
                        let target = path(&name)?;
                        let source = path(&old)?;
                        if source != target {
                            anyhow::ensure!(!target.exists(), "That name already exists");
                            std::fs::rename(source, target)?;
                        }
                        Ok(())
                    } else {
                        write(&name, &Snapshot::capture(self), false)
                    }
                })();
                if result.is_ok() {
                    self.workspaces.dialog = None;
                }
                self.workspace_result(result, "Workspace saved");
            }
        }
        if let Some(name) = self.workspaces.delete.clone() {
            let mut action = 0;
            egui::Window::new("Delete workspace?")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Remove “{name}”? Your files and collections are kept."
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
            if action != 0 {
                self.workspaces.delete = None;
                if action == 1 {
                    let result = path(&name).and_then(|p| Ok(std::fs::remove_file(p)?));
                    self.workspace_result(result, "Workspace deleted");
                }
            }
        }
        if let Some(name) = self.workspaces.restore.take() {
            let result = (|| {
                let text = std::fs::read_to_string(path(&name)?)?;
                anyhow::ensure!(text.len() < 8 * 1024 * 1024, "Workspace is too large");
                self.restore_workspace(Snapshot::parse(&text)?, ctx)
            })();
            if let Err(e) = result {
                self.workspace_result(Err(e), "");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder() -> PathBuf {
        std::env::temp_dir().join(format!(
            "view-calipers-workspace-fixture-{}",
            std::process::id()
        ))
    }
    #[test]
    fn restores_both_panes_tabs_locks_views_and_selection_after_disk_roundtrip() {
        let root = folder();
        std::fs::create_dir_all(root.join("Other")).unwrap();
        for name in ["first.md", "second.md"] {
            std::fs::write(root.join(name), "# Reference").unwrap();
        }
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.select(Node::Dir(root.clone()), &ctx);
        app.goto(1, &ctx);
        app.files.selected.insert(
            app.files
                .rows
                .iter()
                .position(|r| r.path.as_ref() == Some(&root.join("second.md")))
                .unwrap(),
        );
        app.files.details = true;
        app.thumb_size = 240.0;
        app.files.sort = file_browser::Sort {
            key: file_browser::SortKey::Size,
            descending: true,
        };
        app.files.sort_rows();
        app.files.columns.resize(file_browser::SortKey::Name, 100.0);
        app.new_tab(true, &ctx);
        app.select(Node::Dir(root.join("Other")), &ctx);
        app.tabs[1].locked = true;
        app.switch_tab(0);
        app.set_dual(true, &ctx);
        app.set_split_preview(true, &ctx);
        app.switch_pane(1);
        app.select(Node::Dir(root.join("Other")), &ctx);
        app.new_tab(false, &ctx);
        app.select(Node::Dir(root.clone()), &ctx);
        app.goto(0, &ctx);
        app.thumb_size = 96.0;
        app.tabs[0].locked = true;
        app.settings.tree_width = 301.0;
        app.settings.preview_width = 470.0;
        app.pane_fraction = 0.4;
        app.preview_fraction = 0.65;
        app.settings.compare_fullscreen = true;
        let expected = Snapshot::capture(&app);
        let name = "Тест workspace snapshot";
        let _ = std::fs::remove_file(path(name).unwrap());
        write(name, &expected, false).unwrap();
        assert!(write(name, &expected, false).is_err());
        assert!(names().unwrap().contains(&name.to_string()));
        let loaded =
            Snapshot::parse(&std::fs::read_to_string(path(name).unwrap()).unwrap()).unwrap();
        let mut restored = App::new(None);
        restored.settings.photo_ram_mb = 1234;
        restored.restore_workspace(loaded, &ctx).unwrap();
        assert_eq!(restored.active_pane, 1);
        assert_eq!(restored.active_tab, 1);
        assert_eq!(restored.tabs.len(), 2);
        assert!(restored.tabs[0].locked);
        assert_eq!(restored.thumb_size, 96.0);
        assert_eq!(restored.current, Some(0));
        assert!(restored.split_preview && restored.settings.compare_fullscreen);
        assert_eq!(restored.settings.photo_ram_mb, 1234);
        assert_eq!(restored.pane_fraction, 0.4);
        assert_eq!(restored.preview_fraction, 0.65);
        assert_eq!(restored.settings.tree_width, 301.0);
        assert_eq!(restored.settings.preview_width, 470.0);
        restored.switch_pane(0);
        assert_eq!(restored.active_tab, 0);
        assert!(restored.tabs[1].locked);
        assert!(restored.files.details);
        assert_eq!(restored.thumb_size, 240.0);
        assert!(restored.files.sort.descending);
        assert_eq!(
            restored.files.sort.key as u8,
            file_browser::SortKey::Size as u8
        );
        assert_eq!(restored.files.columns.columns[0].width, 370.0);
        assert_eq!(restored.current, Some(1));
        assert!(restored
            .files
            .selected
            .iter()
            .any(|i| restored.files.rows[*i].path.as_ref() == Some(&root.join("second.md"))));
        restored.switch_tab(1);
        assert_eq!(restored.selected, Some(Node::Dir(root.join("Other"))));
        assert_eq!(restored.tabs.len(), 2);
        std::fs::remove_file(path(name).unwrap()).unwrap();
        app.retire_workspace();
        restored.retire_workspace();
    }
    #[test]
    fn collections_missing_locations_and_corrupt_records_are_safe() {
        let _library = crate::collections::test_guard();
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        let root = folder();
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("workspace-reference.md");
        std::fs::write(&original, "# Original").unwrap();
        let id = collections::create("Workspace references", &[original.clone()]).unwrap();
        app.select(
            Node::Collection {
                id: id.clone(),
                name: "Workspace references".into(),
            },
            &ctx,
        );
        app.goto(0, &ctx);
        let mut saved = Snapshot::capture(&app);
        saved.panes[0].tabs[0].locked = true;
        app.restore_workspace(Snapshot::parse(&saved.text()).unwrap(), &ctx)
            .unwrap();
        assert!(matches!(&app.selected,Some(Node::Collection{id:got,..}) if got==&id));
        assert_eq!(app.current, Some(0));
        assert!(app.tabs[0].locked);
        saved.panes[0].tabs[0].node = Node::Dir(root.join("unavailable-location"));
        app.restore_workspace(saved, &ctx).unwrap();
        assert_eq!(app.selected, Some(Node::Computer));
        assert!(app.file_message.contains("unavailable"));
        let valid = Snapshot::capture(&app).text();
        assert!(Snapshot::parse("broken").is_err());
        assert!(Snapshot::parse(&valid.replace("A\t0", "A\t4")).is_err());
        assert!(Snapshot::parse(&valid.replace("P\t0\t0", "P\t0\t99")).is_err());
        assert!(Snapshot::parse(&(valid + "bad record\n")).is_err());
        assert!(valid_name(" \n").is_err());
        assert!(valid_name(&"a".repeat(81)).is_err());
        assert_eq!(std::fs::read_to_string(original).unwrap(), "# Original");
        collections::update(|library| {
            library.collections.retain(|c| c.id != id);
            Ok(())
        })
        .unwrap();
        app.retire_workspace();
    }
}
