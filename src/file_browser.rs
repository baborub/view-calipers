use super::*;
use crate::{clipboard::Files, fileops::Job};

#[derive(Clone)]
pub struct Row {
    pub(super) bytes:Option<u64>,
    pub(super) modified:String,
    pub(super) label: String,
    pub(super) path: Option<PathBuf>,
    pub(super) node: Option<Node>,
    pub(super) media: Option<usize>,
    pub(super) thumb: Option<usize>,
    pub(super) size: String,
    pub(super) created: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SortKey { #[default] Name, Type, Size, Created, Modified }
impl SortKey {
    pub fn label(self) -> &'static str { match self { Self::Name=>"Name",Self::Type=>"Type",Self::Size=>"Size",Self::Created=>"Created",Self::Modified=>"Modified" } }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort { pub key: SortKey, pub descending: bool }
impl Sort {
    pub fn sdk(self) -> u32 { (match self.key { SortKey::Name=>1,SortKey::Type=>7,SortKey::Size=>5,SortKey::Created=>11,SortKey::Modified=>13 }) + u32::from(self.descending) }
}
impl Row {
    fn folder(&self) -> bool { matches!(self.node,Some(Node::Dir(_) | Node::Computer | Node::ArchiveDir{..})) }
    pub(super) fn extension(&self)->String{self.path.as_deref().map(ext_of_path).unwrap_or_else(||ext_of_name(&self.label))}
    fn kind(&self) -> String { if self.folder() { "Folder".into() } else { let ext=self.extension(); if ext.is_empty(){"File".into()}else{ext.to_uppercase()} } }
}
impl State {
    pub fn sort_rows(&mut self) {
        // Everything sorts the complete index before pagination.
        if self.search.results { return; }
        let old=std::mem::take(&mut self.rows);
        let mut rows:Vec<_>=old.into_iter().enumerate().collect();
        let sort=self.sort;
        let order:HashMap<_,_>=self.collection_order.iter().enumerate().map(|(i,p)|(p,i)).collect();
        rows.sort_by(|(_,a),(_,b)| {
            if self.collection.is_some()&&self.collection_custom{return a.path.as_ref().and_then(|p|order.get(p)).cmp(&b.path.as_ref().and_then(|p|order.get(p)));}
            let folders=b.folder().cmp(&a.folder());
            if folders!=Ordering::Equal && !self.search.results { return folders; }
            let cmp=match sort.key { SortKey::Name=>natural_cmp(&a.label,&b.label),SortKey::Type=>natural_cmp(&a.kind(),&b.kind()),SortKey::Size=>a.bytes.cmp(&b.bytes),SortKey::Created=>a.created.cmp(&b.created),SortKey::Modified=>a.modified.cmp(&b.modified) };
            let cmp=cmp.then_with(||natural_cmp(&a.label,&b.label));
            if sort.descending { cmp.reverse() } else { cmp }
        });
        let selected=self.selected.clone(); let anchor=self.anchor;
        self.selected=rows.iter().enumerate().filter_map(|(new,(old,_))|selected.contains(old).then_some(new)).collect();
        self.anchor=rows.iter().position(|(old,_)|Some(*old)==anchor);
        self.rows=rows.into_iter().map(|(_,row)|row).collect();
        self.marquee=None;
    }
}
impl App {
    fn change_sort(&mut self,sort:Sort,ctx:&egui::Context) {
        self.files.collection_custom=false;
        self.files.sort=sort;
        if self.files.search.results { self.files.search.offset=0;self.submit_search(ctx); }
        else { self.files.sort_rows(); }
    }
}

#[derive(Clone)]
pub enum Command {
    Copy(bool),
    Paste,
    Duplicate,
    Delete,
    Rename,
    NewFolder,
    Other(bool),
    Drop(Files, PathBuf),
}
#[derive(Default)]
pub struct State {
    pub new_board_sources:Vec<ImageSrc>,
    #[cfg(test)]
    pub(crate) item_rects: Vec<Rect>,
    pub(super) collection:Option<String>,
    pub(super) collection_order:Vec<PathBuf>,
    pub(super) collection_custom:bool,
    pub(super) collection_revision:u64,
    pub(super) collection_dialog:Option<(Option<String>,String,Vec<PathBuf>)>,
    pub details: bool,
    pub sort: Sort,
    pub columns: crate::folder_views::Layout,
    pub(super) view_folder: Option<PathBuf>,
    view_saved: Option<crate::folder_views::View>,
    view_pending: Option<(crate::folder_views::View,std::time::Instant)>,
    new_folder: Option<(PathBuf,String)>,
    pub address: String,
    pub focus_address: bool,
    pub(super) rows: Vec<Row>,
    pub(super) all_rows:Vec<Row>,
    pub search:search::State,
    pub(super) selected: HashSet<usize>,
    pub(super) anchor: Option<usize>,
    pub(super) reveal_row: Option<usize>,
    pub pending: Option<Command>,
    rename: Option<(PathBuf, String)>,

    pub(super) marquee: Option<Marquee>,
}
fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else {
        format!("{:.2} GB", bytes as f64 / 1073741824.0)
    }
}
fn created(path: &Path) -> String {
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(path) { return everything::format_time(meta.creation_time()); }
    }
    "—".into()
}

impl App {
    pub fn refresh_file_rows(&mut self) {
        self.files.collection=match &self.selected{Some(Node::Collection{id,..})=>Some(id.clone()),_=>None};
        self.files.collection_order=self.files.collection.as_deref().and_then(collections::get).map(|c|c.paths).unwrap_or_default();
        self.files.collection_revision=collections::revision();
        let mut rows = Vec::new();
        let mut known = HashSet::new();
        for (i, node) in self.gallery_nodes.iter().enumerate() {
            let path = match node {
                Node::Dir(p) | Node::Archive(p) => Some(p.clone()),
                _ => None,
            };
            if let Some(p) = &path {
                known.insert(p.clone());
            }
            rows.push(Row {
                bytes:None,modified:String::new(),
                label: node.label(),
                path,
                node: Some(node.clone()),
                media: None,
                thumb: Some(self.images.len() + i),
                size: String::new(),
                created: String::new(),
            });
        }
        for (i, image) in self.images.iter().enumerate() {
            let path = match &image.src {
                ImageSrc::File(p) => Some(p.clone()),
                _ => None,
            };
            if let Some(p) = &path {
                known.insert(p.clone());
            }
            rows.push(Row {
                bytes:None,modified:String::new(),
                label: image.label.clone(),
                path,
                node: None,
                media: Some(i),
                thumb: Some(i),
                size: String::new(),
                created: String::new(),
            });
        }
        if let Some(Node::Dir(dir)) = &self.selected {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut other: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| !known.contains(p) && ext_of_path(p)!="vcboard" && file_name(p)!=".collection-id")
                    .collect();
                other.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
                for path in other {
                    rows.push(Row {
                bytes:None,modified:String::new(),
                        label: file_name(&path),
                        path: Some(path),
                        node: None,
                        media: None,
                        thumb: None,
                        size: String::new(),
                        created: String::new(),
                    });
                }
            }
        }
        for path in &self.files.collection_order{if !known.contains(path){rows.push(Row{label:if path.exists(){file_name(path)}else{format!("{} [missing]",file_name(path))},path:Some(path.clone()),node:None,media:None,thumb:None,bytes:None,size:String::new(),created:String::new(),modified:String::new()});}}
        for row in &mut rows {
            if let Some(path) = &row.path {
                let metadata=std::fs::metadata(path).ok();row.bytes=metadata.as_ref().filter(|m|m.is_file()).map(|m|m.len());
                #[cfg(windows)]{use std::os::windows::fs::MetadataExt;row.modified=metadata.as_ref().map(|m|everything::format_time(m.last_write_time())).unwrap_or_default();}
                row.size = std::fs::metadata(path)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| size(m.len()))
                    .unwrap_or_else(|| "—".into());
                row.created = created(path);
            } else {
                row.size = "—".into();
                row.created = "—".into();
            }
        }
        self.files.marquee = None;
        self.files.rows = rows;
        self.files.all_rows=self.files.rows.clone();
        if self.files.search.scope==0{self.apply_file_filter();}else{self.files.sort_rows();}
        self.files.selected.clear();
        self.files.anchor = None;
        self.files.address = match &self.selected {
            Some(Node::Dir(p) | Node::Archive(p)) => p.display().to_string(),
            Some(Node::ArchiveDir { archive, .. }) => archive.display().to_string(),
            Some(Node::Collection{name,..})=>format!("Collection: {name}"),
            _ => "This PC".into(),
        };
    }
    pub(super) fn selected_paths(&self) -> Vec<PathBuf> {
        let mut indices: Vec<_> = self.files.selected.iter().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|i| self.files.rows.get(i)?.path.clone())
            .collect()
    }
    pub(super) fn selected_board_sources(&self) -> Vec<ImageSrc> {
        let mut indices: Vec<_> = self.files.selected.iter().copied().collect();
        indices.sort_unstable();
        indices.into_iter().filter_map(|i| {
            let media = self.files.rows.get(i)?.media?;
            let source = &self.images.get(media)?.src;
            board::supports_source(source).then(|| source.clone())
        }).collect()
    }
    pub fn file_address(&mut self, ui: &mut egui::Ui) {
        let mut go = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.files.address)
                    .desired_width((ui.available_width() - 45.0).max(50.0))
                    .hint_text("Paste a folder or file path · Enter"),
            );
            if self.files.focus_address {
                response.request_focus();
                let mut state =
                    egui::TextEdit::load_state(ui.ctx(), response.id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(self.files.address.chars().count()),
                    )));
                state.store(ui.ctx(), response.id);
                self.files.focus_address = false;
            }
            go = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Go").clicked() {
                go = true;
            }
        });
        if go {
            let text = self.files.address.trim().trim_matches('"');
            if text.eq_ignore_ascii_case("This PC") {
                self.select(Node::Computer, ui.ctx());
            } else {
                let path = PathBuf::from(text);
                let path = if path.is_absolute() {
                    path
                } else {
                    match &self.selected {
                        Some(Node::Dir(p)) => p.join(path),
                        _ => path,
                    }
                };
                let path = std::path::absolute(&path).unwrap_or(path);
                if path.exists() {
                    self.open_path(&path, ui.ctx());
                    if let Some(index) = self
                        .files
                        .rows
                        .iter()
                        .position(|row| row.path.as_ref() == Some(&path))
                    {
                        self.files.select_only(index);
                    }
                } else {
                    self.status = "Path does not exist or is unavailable".into();
                    self.file_message = self.status.clone();
                }
            }
        }
    }
    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let paths=self.selected_paths();let available=!paths.is_empty();
        if ui.button("New collection from selection…").on_hover_text("Create a canvas from selected images, videos and Markdown files").clicked(){self.create_board(self.selected_board_sources(),ui.ctx());ui.close_menu();}
        if let Some(id)=self.files.collection.clone(){if ui.button("Show collection canvas").clicked(){self.open_collection_board(&id,ui.ctx());ui.close_menu();}}
        ui.add_enabled_ui(available,|ui|{ui.menu_button("Add to collection",|ui|{
            if ui.button("New collection…").clicked(){self.files.collection_dialog=Some((None,String::new(),paths.clone()));ui.close_menu();}
            for (id,name,_) in collections::list(){if ui.button(name).clicked(){self.collection_message(collections::add(&id,&paths));ui.close_menu();}}
        });});
        if self.files.collection.is_some(){ui.small("Delete removes references only; original files are kept.");ui.separator();}

        if self.files.search.results || self.files.collection.is_some() {let path=self.selected_paths().first().cloned();if ui.add_enabled(path.is_some(),egui::Button::new("Show in folder")).clicked(){if let Some(path)=path{self.reveal_original(&path,ui.ctx());}ui.close_menu();}ui.separator();}

        if !self.files.search.results && self.selected.as_ref().is_some_and(Node::is_archive_like) {
            ui.label("Archive contents are read-only");
        }
        if ui.add_enabled(self.destination().is_some(),egui::Button::new("New folder · Ctrl+Shift+N")).clicked(){self.files.pending=Some(Command::NewFolder);ui.close_menu();}
        for (label, command) in [
            ("Copy · Ctrl+C", Command::Copy(false)),
            ("Cut · Ctrl+X", Command::Copy(true)),
            ("Duplicate file(s) · Ctrl+Shift+D", Command::Duplicate),
            ("Rename · F2", Command::Rename),
            (if self.files.collection.is_some(){"Remove from collection · Delete"}else{"Delete · Delete"}, Command::Delete),
            ("Copy to other pane", Command::Other(false)),
            ("Move to other pane", Command::Other(true)),
        ] {
            if ui
                .add_enabled(available, egui::Button::new(label))
                .clicked()
            {
                self.files.pending = Some(command);
                ui.close_menu();
            }
        }
        if ui
            .add_enabled(
                self.destination().is_some(),
                egui::Button::new("Paste · Ctrl+V"),
            )
            .clicked()
        {
            self.files.pending = Some(Command::Paste);
            ui.close_menu();
        }
    }
    fn sort_menu(&mut self, ui: &mut egui::Ui) {
        let mut sort=self.files.sort;
        let custom=self.files.collection.is_some()&&self.files.collection_custom;
        if self.files.collection.is_some(){if ui.selectable_label(custom,"Collection order").clicked(){self.files.collection_custom=true;self.files.sort_rows();ui.close_menu();}ui.separator();}
        let mut normal=false;
        for key in [SortKey::Name,SortKey::Type,SortKey::Size,SortKey::Created,SortKey::Modified] {normal|=ui.selectable_value(&mut sort.key,key,key.label()).clicked();}
        ui.separator();ui.selectable_value(&mut sort.descending,false,"Ascending");ui.selectable_value(&mut sort.descending,true,"Descending");
        if sort!=self.files.sort||normal{self.change_sort(sort,ui.ctx());}
    }
    fn columns_menu(&mut self, ui: &mut egui::Ui) {
        let snapshot=self.files.columns.columns.clone();
        for column in snapshot {ui.horizontal(|ui|{
            if let Some(c)=self.files.columns.columns.iter_mut().find(|c|c.key==column.key){ui.add_enabled(c.key!=SortKey::Name,egui::Checkbox::new(&mut c.visible,c.key.label()));}
            let index=self.files.columns.columns.iter().position(|c|c.key==column.key).unwrap();
            if ui.add_enabled(index>0,egui::Button::new("↑")).on_hover_text("Move column left").clicked(){self.files.columns.columns.swap(index,index-1);}
            if ui.add_enabled(index+1<5,egui::Button::new("↓")).on_hover_text("Move column right").clicked(){self.files.columns.columns.swap(index,index+1);}
        });}
        ui.separator();if ui.button("Reset columns").clicked(){self.files.columns=crate::folder_views::Layout::default();}
        ui.small("Drag headers to reorder; drag their right edge to resize.");
    }
    pub(super) fn file_toolbar(&mut self, ui: &mut egui::Ui) {
        use crate::ui_theme::{self as theme, Icon};
        let width=ui.available_width();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x=4.0;
            if theme::button(ui,Icon::Grid,"Thumbnails",true,!self.files.details).clicked(){self.files.details=false;}
            if theme::button(ui,Icon::List,"Details",true,self.files.details).clicked(){self.files.details=true;}
            theme::icon_menu(ui,Icon::More,"File actions",false,|ui|{
                self.file_menu(ui);
                if width<250.0 {ui.separator();ui.menu_button("Columns",|ui|self.columns_menu(ui));}
                ui.separator();ui.label("Thumbnail size");ui.add(egui::Slider::new(&mut self.thumb_size,80.0..=320.0).suffix(" px"));
            });
            if width>=250.0 && theme::button(ui,Icon::NewFolder,"New folder · Ctrl+Shift+N",self.destination().is_some()&&self.file_job.is_none(),false).clicked(){self.files.pending=Some(Command::NewFolder);}
            let custom=self.files.collection.is_some()&&self.files.collection_custom;
            let label=if custom{"Collection order".into()}else{format!("{} {}",self.files.sort.key.label(),if self.files.sort.descending{"↓"}else{"↑"})};
            if width>=420.0 {ui.menu_button(format!("Sort: {label}"),|ui|self.sort_menu(ui)).response.on_hover_text("Sort files");}
            else {theme::icon_menu(ui,Icon::Sort,&format!("Sort: {label}"),false,|ui|self.sort_menu(ui));}
            if width>=250.0 {theme::icon_menu(ui,Icon::Columns,"Details columns",false,|ui|self.columns_menu(ui));}
            if width>=650.0 {
                let selected=!self.selected_paths().is_empty();
                if theme::button(ui,Icon::Copy,"Copy to other pane",selected,false).clicked(){self.files.pending=Some(Command::Other(false));}
                if theme::button(ui,Icon::Move,"Move to other pane · Ctrl+drag to copy",selected,false).clicked(){self.files.pending=Some(Command::Other(true));}
            }
            if !self.files.details && width>=300.0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                    ui.spacing_mut().slider_width=(ui.available_width()-8.0).clamp(30.0,110.0);
                    ui.add(egui::Slider::new(&mut self.thumb_size,80.0..=320.0).show_value(false)).on_hover_text(format!("Thumbnail size: {:.0} px",self.thumb_size));
                });
            }
        });
        ui.add_space(2.0);
    }
    pub fn file_gallery(&mut self, ui: &mut egui::Ui) {
        self.file_toolbar(ui);
        self.search_bar(ui);
        let details = self.files.details;
        let detail_columns=self.files.columns.visible();
        let content_width=(ui.available_width()-ui.spacing().scroll.allocated_width()).max(1.0);
        let width = if details {
            detail_columns.iter().map(|c|c.width).sum::<f32>().max(content_width)
        } else {
            content_width
        };
        let tile = (self.thumb_size + 10.0).min(width).max(1.0);
        let height = if details {
            crate::ui_theme::row_height(self.settings.comfortable)
        } else {
            (tile - 10.0) * 0.75 + 35.0
        };
        let columns = if details {
            1
        } else {
            ((width + ui.spacing().item_spacing.x) / (tile + ui.spacing().item_spacing.x))
                .floor()
                .max(1.0) as usize
        };
        let count = self.files.rows.len();
        let area = ui.available_rect_before_wrap();
        let mut clicked = None;
        let mut open = None;
        let mut visible = Vec::new();
        let mut origin = None;
        let spacing = ui.spacing().item_spacing;
        if count == 0 {
            ui.label(
                self.selected
                    .as_ref()
                    .and_then(|n| match n {
                        Node::Archive(p) | Node::ArchiveDir { archive: p, .. } => {
                            self.archive_errors.get(p)
                        }
                        _ => None,
                    })
                    .map(String::as_str)
                    .unwrap_or(if self.files.search.results{"No search results"}else if !self.files.search.query.is_empty()||self.files.search.kind!=0{"No files match the filter"}else{"Folder is empty"}),
            );
        }
        let mut scroll_area = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .id_salt("file_gallery");
        if let Some(row) = self.files.reveal_row.take().filter(|row| *row < count) {
            scroll_area = scroll_area.vertical_scroll_offset((row / columns + usize::from(details)) as f32 * (height + spacing.y));
        }
        let scroll = scroll_area.show_rows(
                ui,
                height,
                count.div_ceil(columns) + usize::from(details),
                |ui, range| {
                    if details && range.start == 0 {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
                        let mut x=rect.left();
                        for column in &detail_columns {
                            let key=column.key;
                            let cell=Rect::from_min_max(Pos2::new(x,rect.top()),Pos2::new(x+column.width,rect.bottom()));x+=column.width;
                            let body=Rect::from_min_max(cell.min+Vec2::new(3.0,0.0),cell.max-Vec2::new(5.0,0.0));
                            let response=ui.interact(body,ui.id().with(("sort_column",key as u8)),Sense::click_and_drag());
                            response.dnd_set_drag_payload(key);
                            if let Some(source)=response.dnd_release_payload::<SortKey>(){self.files.columns.reorder(*source,key);}
                            if response.dnd_hover_payload::<SortKey>().is_some(){ui.painter().rect_stroke(cell,0.0,egui::Stroke::new(1.0_f32,Color32::LIGHT_BLUE));}
                            if response.clicked(){let descending=self.files.sort.key==key && !self.files.sort.descending;self.change_sort(Sort{key,descending},ui.ctx());}
                            let suffix=if self.files.sort.key==key && !(self.files.collection.is_some()&&self.files.collection_custom) {if self.files.sort.descending{" ↓"}else{" ↑"}}else{""};
                            ui.painter().with_clip_rect(body).text(cell.left_center()+Vec2::new(6.0,0.0),Align2::LEFT_CENTER,format!("{}{suffix}",key.label()),FontId::proportional(12.0),Color32::LIGHT_GRAY);
                            response.on_hover_text(format!("Sort by {} · click to reverse · drag to reorder",key.label()));
                            let edge=Rect::from_min_max(Pos2::new(cell.right()-5.0,cell.top()),Pos2::new(cell.right()+3.0,cell.bottom()));
                            let resize=ui.interact(edge,ui.id().with(("resize_column",key as u8)),Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                            if resize.dragged(){self.files.columns.resize(key,ui.input(|i|i.pointer.delta().x));}
                            if resize.double_clicked(){if let Some(default)=crate::folder_views::Layout::default().columns.iter().find(|c|c.key==key){if let Some(c)=self.files.columns.columns.iter_mut().find(|c|c.key==key){c.width=default.width;}}}
                            ui.painter().vline(cell.right(),cell.y_range(),egui::Stroke::new(1.0_f32,Color32::from_gray(55)));
                        }
                    }
                    let range = if details {
                        range.start.saturating_sub(1)..range.end.saturating_sub(1)
                    } else {
                        range
                    };
                    if let Some(queue) = &self.thumb_queue {
                        let mut queue = queue.lock().unwrap();
                        for index in (range.start * columns..(range.end * columns).min(count)).rev()
                        {
                            if let Some(t) = self.files.rows[index].thumb {
                                if let Some(i) = queue.iter().position(|job| *job == t) {
                                    queue.remove(i);
                                    queue.push_front(t);
                                }
                            }
                        }
                    }
                    for row_index in range {
                        ui.horizontal(|ui| {
                            for index in row_index * columns..((row_index + 1) * columns).min(count)
                            {
                                let row = self.files.rows[index].clone();
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(if details { width } else { tile }, height),
                                    Sense::click_and_drag(),
                                );
                                visible.push(rect);
                                origin = Some(
                                    rect.min
                                        - Vec2::new(
                                            (index % columns) as f32 * (tile + spacing.x),
                                            (index / columns) as f32 * (height + spacing.y),
                                        ),
                                );
                                let selected = self.files.selected.contains(&index);
                                let painter = ui.painter();
                                painter.rect_filled(
                                    rect,
                                    7.0,
                                    if selected {
                                        crate::ui_theme::SELECTED
                                    } else if response.hovered() {
                                        crate::ui_theme::HOVER
                                    } else {
                                        crate::ui_theme::RAISED
                                    },
                                );
                                if selected {painter.rect_stroke(rect.shrink(0.5),7.0,egui::Stroke::new(1.0_f32,crate::ui_theme::ACCENT.gamma_multiply(0.65)));}
                                if details {
                                    let mut x=rect.left();
                                    for column in &detail_columns {
                                        let text=match column.key {SortKey::Name=>row.label.clone(),SortKey::Type=>row.kind(),SortKey::Size=>row.size.clone(),SortKey::Created=>row.created.clone(),SortKey::Modified=>row.modified.clone()};
                                        let cell=Rect::from_min_max(Pos2::new(x,rect.top()),Pos2::new(x+column.width-5.0,rect.bottom()));x+=column.width;
                                        painter.with_clip_rect(cell).text(cell.left_center()+Vec2::new(6.0,0.0),Align2::LEFT_CENTER,text,FontId::proportional(12.0),Color32::LIGHT_GRAY);
                                    }
                                } else {
                                    let preview = Rect::from_min_size(
                                        rect.min + Vec2::splat(5.0),
                                        Vec2::new((tile - 10.0).max(1.0), (height - 35.0).max(1.0)),
                                    );
                                    let kind = match &row.node {
                                        Some(Node::Dir(_)) => "Folder",
                                        Some(Node::Archive(_) | Node::ArchiveDir { .. }) => {
                                            "Archive"
                                        }
                                        _ => {
                                            if row.media.is_some() {
                                                "Media"
                                            } else {
                                                "File"
                                            }
                                        }
                                    };
                                    if let Some(texture) = row
                                        .thumb
                                        .and_then(|i| self.thumbs.get(i))
                                        .and_then(Option::as_ref)
                                    {
                                        let size = texture.size_vec2();
                                        let scale = (preview.width() / size.x)
                                            .min(preview.height() / size.y);
                                        painter.image(
                                            texture.id(),
                                            Rect::from_center_size(preview.center(), size * scale),
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        crate::ui_theme::paint(
                                            painter,
                                            if kind == "Folder" || kind == "Archive" {
                                                crate::ui_theme::Icon::Folder
                                            } else {
                                                crate::ui_theme::Icon::File
                                            },
                                            Rect::from_center_size(
                                                preview.center(),
                                                Vec2::splat(40.0),
                                            ),
                                            crate::ui_theme::MUTED,
                                        );
                                    }
                                    let badge = match &row.node {
                                        Some(Node::Dir(_)) => Some("Folder".to_string()),
                                        Some(Node::Archive(_) | Node::ArchiveDir { .. }) => {
                                            Some("Archive".to_string())
                                        }
                                        _ => row.path.as_ref().and_then(|p| {
                                            let ext = ext_of_path(p);
                                            if VIDEO_EXT.contains(&ext.as_str()) {
                                                Some("Video".into())
                                            } else if DOCUMENT_EXT.contains(&ext.as_str())
                                                || model3d::EXTENSIONS.contains(&ext.as_str())
                                            {
                                                Some(ext.to_uppercase())
                                            } else {
                                                None
                                            }
                                        }),
                                    };
                                    if let Some(badge) = badge {
                                        let galley=painter.layout_no_wrap(badge,FontId::proportional(11.0),crate::ui_theme::TEXT);
                                        let pill=Rect::from_min_size(preview.right_bottom()-galley.size()-Vec2::new(12.0,7.0),galley.size()+Vec2::new(10.0,4.0));
                                        painter.rect_filled(pill,4.0,crate::ui_theme::BACKGROUND.gamma_multiply(0.9));
                                        painter.galley(pill.min+Vec2::new(5.0,2.0),galley,crate::ui_theme::TEXT);
                                    }
                                    let label = Rect::from_min_max(
                                        Pos2::new(rect.left() + 6.0, rect.bottom() - 27.0),
                                        rect.max - Vec2::new(6.0, 3.0),
                                    );
                                    painter.with_clip_rect(label).text(
                                        label.left_center(),
                                        Align2::LEFT_CENTER,
                                        &row.label,
                                        FontId::proportional(crate::ui_theme::font_size(
                                            self.settings.comfortable,
                                        )),
                                        Color32::WHITE,
                                    );
                                }
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if response.double_clicked() {
                                    open = Some(index);
                                }
                                if response.secondary_clicked() && !selected {
                                    self.files.selected.clear();
                                    self.files.selected.insert(index);
                                }
                                response.context_menu(|ui| self.file_menu(ui));
                                let paths = if selected {
                                    self.selected_paths()
                                } else {
                                    row.path.clone().into_iter().collect()
                                };
                                if response.drag_started() && !selected {
                                    self.files.select_only(index);
                                }
                                if !paths.is_empty() {
                                    response.dnd_set_drag_payload(Files { paths, cut: true });
                                } else if response.drag_started() {
                                    let sources = self.selected_board_sources();
                                    if !sources.is_empty() {
                                        response.dnd_set_drag_payload(board::ImageDrag { sources });
                                    }
                                }
                                if let (Some(id),Some(target))=(self.files.collection.clone(),row.path.as_ref()){
                                    if let Some(files)=response.dnd_release_payload::<Files>(){self.files.collection_custom=true;self.collection_message(collections::reorder(&id,&files.paths,target,ui.input(|i|i.pointer.hover_pos().is_some_and(|p|if details{p.y>rect.center().y}else{p.x>rect.center().x}))));}
                                }
                                if let Some(Node::Dir(dir)) = &row.node {
                                    self.folder_drop_target(ui, &response, dir);
                                }
                                response.on_hover_text(format!(
                                    "{}\n{} · {}",
                                    row.path
                                        .as_ref()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or(row.label),
                                    row.size,
                                    row.created
                                ));
                            }
                        });
                    }
                },
            );
        #[cfg(test)]
        { self.files.item_rects = visible.clone(); }
        if let Some(origin) = origin {
            self.marquee_selection(
                ui,
                scroll.inner_rect,
                origin,
                Vec2::new(if details { width } else { tile }, height),
                Vec2::new(
                    if details {
                        width + spacing.x
                    } else {
                        tile + spacing.x
                    },
                    height + spacing.y,
                ),
                columns,
                count,
                &visible,
            );
        }
        if let Some(index) = clicked {
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.shift {
                let anchor = self.files.anchor.unwrap_or(index);
                if !modifiers.ctrl {
                    self.files.selected.clear();
                }
                self.files
                    .selected
                    .extend(anchor.min(index)..=anchor.max(index));
            } else if modifiers.ctrl {
                if !self.files.selected.remove(&index) {
                    self.files.selected.insert(index);
                }
                self.files.anchor = Some(index);
            } else {
                self.files.selected.clear();
                self.files.selected.insert(index);
                self.files.anchor = Some(index);
                if let Some(media) = self.files.rows[index].media {
                    if self.current != Some(media) {
                        self.goto(media, ui.ctx());
                    }
                } else {
                    if !self.close_board() { return; }
                    self.photos.cancel();
                    self.current = None;
                    self.texture = None;
                    self.video = None;
                    self.scrubber = None;
                    self.video_seek = None;
                    self.pdf = None;
                    self.pdf_scrubber = None;
                    self.pdf_request = None;
                    self.markdown = None;
                    self.model = None;
                    self.model_request = None;
                    self.model_loader = None;
                    self.model_error = None;
                }
            }
        }
        if let Some(index) = open {
            let row = self.files.rows[index].clone();
            if let Some(node) = row.node {
                self.select(node, ui.ctx());
            } else if let Some(media) = row.media {
                if self.current != Some(media) {
                    self.goto(media, ui.ctx());
                }
                self.set_fullscreen(ui.ctx(), true);
            }
        }
        if let Some(id)=self.files.collection.clone(){let response=ui.interact(area,ui.id().with("add_collection_references"),Sense::hover());if let Some(files)=response.dnd_release_payload::<Files>(){self.collection_message(collections::add(&id,&files.paths));}}
        self.save_folder_view(false);
        if self.files.view_pending.is_some(){ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));}
        if let Some(dir) = self.destination() {
            let response = ui.interact(area, ui.id().with("drop_into_folder"), Sense::hover());
            self.folder_drop_target(ui, &response, &dir);
        }
    }
    fn destination(&self) -> Option<PathBuf> {
        if self.files.search.results {return None;}
        match &self.selected {
            Some(Node::Dir(p)) => Some(p.clone()),
            _ => None,
        }
    }
    pub fn file_commands(&mut self, ctx: &egui::Context) {
        if self.files.collection_dialog.is_none() && self.inactive.as_ref().is_some_and(|other|other.files.collection_dialog.is_some()){self.switch_pane(1-self.active_pane);}
        self.collection_dialog(ctx);
        if let Some(paths) = self.workbench.recycle.as_ref() {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete selected items?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!("Move {} item(s) to the Recycle Bin?", paths.len()));
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for path in paths {
                                ui.label(path.display().to_string());
                            }
                        });
                    ui.horizontal(|ui| {
                        confirm = ui.button("Move to Recycle Bin").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            cancel |= ctx.input(|i| i.key_pressed(Key::Escape));
            if confirm {
                let paths = self.workbench.recycle.take().unwrap();
                self.start_file_job(Job::Recycle(paths), ctx);
            } else if cancel {
                self.workbench.recycle = None;
            }
        }

        if !self.board_focus() && !ctx.wants_keyboard_input()
            && !self.workspace_modal()
            && !self.settings_open
            && !self.fullscreen
            && !self.workbench.palette
            && self.files.rename.is_none()
            && self.files.new_folder.is_none()
            && self.files.collection_dialog.is_none()
            && self.workbench.recycle.is_none()
        {
            let command = if ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT,Key::N)) {
                Some(Command::NewFolder)
            } else if self.settings.pressed(Action::CopyFiles, ctx) {
                Some(Command::Copy(false))
            } else if self.settings.pressed(Action::CutFiles, ctx) {
                Some(Command::Copy(true))
            } else if self.settings.pressed(Action::PasteFiles, ctx) {
                Some(Command::Paste)
            } else if self.settings.pressed(Action::DuplicateFiles, ctx) {
                Some(Command::Duplicate)
            } else if self.settings.pressed(Action::DeleteFiles, ctx) {
                Some(Command::Delete)
            } else if self.settings.pressed(Action::RenameFile, ctx) {
                Some(Command::Rename)
            } else {
                None
            };
            if command.is_some() {
                self.files.pending = command;
            }
            if self.settings.pressed(Action::Address, ctx) {
                self.files.focus_address = true;
            }
            if self.settings.pressed(Action::SelectAllFiles, ctx) {
                self.files.selected = (0..self.files.rows.len()).collect();
            }
            if self.settings.pressed(Action::DetailsView, ctx) {
                self.files.details = !self.files.details;
            }
        }
        if let Some((directory,name))=&mut self.files.new_folder {
            let mut apply=false;let mut cancel=false;
            egui::Window::new("New folder").collapsible(false).resizable(false).show(ctx,|ui|{
                ui.label(directory.display().to_string());
                let response=ui.text_edit_singleline(name);
                if !response.has_focus(){response.request_focus();}
                if let Err(error)=crate::fileops::valid_name(name){ui.colored_label(Color32::LIGHT_RED,error.to_string());}
                ui.horizontal(|ui|{apply=ui.add_enabled(crate::fileops::valid_name(name).is_ok(),egui::Button::new("Create")).clicked();cancel=ui.button("Cancel").clicked();});
            });
            apply|=ctx.input(|i|i.key_pressed(Key::Enter));cancel|=ctx.input(|i|i.key_pressed(Key::Escape));
            if cancel{self.files.new_folder=None;}else if apply && crate::fileops::valid_name(name).is_ok(){let (dir,name)=self.files.new_folder.take().unwrap();self.start_file_job(Job::NewFolder(dir,name),ctx);}
        }
        if let Some((source, name)) = &mut self.files.rename {
            let mut apply = false;
            let mut cancel = false;
            egui::Window::new("Rename file or folder")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(source.display().to_string());
                    ui.text_edit_singleline(name);
                    ui.horizontal(|ui| {
                        apply = ui.button("Rename").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            apply |= ctx.input(|i| i.key_pressed(Key::Enter));
            cancel |= ctx.input(|i| i.key_pressed(Key::Escape));
            if apply {
                let (source, name) = self.files.rename.take().unwrap();
                self.start_file_job(Job::Rename(source, name), ctx);
            } else if cancel {
                self.files.rename = None;
            }
        }
        if let Some(rx) = &self.file_job {
            match rx.try_recv() {
                Ok(outcome) => {
                    self.file_job = None;
                    if !outcome.moved.is_empty() {
                        if let Ok(mut files) = crate::clipboard::read() {
                            if files.cut {
                                let count = files.paths.len();
                                files
                                    .paths
                                    .retain(|p| !outcome.moved.iter().any(|m| m == p));
                                if files.paths.len() != count {
                                    let _ = crate::clipboard::write(&files);
                                }
                            }
                        }
                    }
                    let message = if outcome.errors.is_empty() {
                        format!("Completed: {} item(s)", outcome.paths.len())
                    } else {
                        outcome.errors.join("\n")
                    };
                    self.refresh_after_files(&outcome, ctx);
                    self.status = message.clone();
                    self.file_message = message;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.file_job = None;
                    self.file_message = "File worker stopped unexpectedly".into();
                }
                _ => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        let mut commands = Vec::new();
        if let Some(command) = self.files.pending.take() {
            commands.push((self.active_pane, command));
        }
        if let Some(other) = self.inactive.as_mut() {
            if let Some(command) = other.files.pending.take() {
                commands.push((1 - self.active_pane, command));
            }
        }
        for (pane, command) in commands {
            let source = if pane == self.active_pane {
                &*self
            } else {
                self.inactive.as_deref().unwrap()
            };
            let paths = source.selected_paths();
            let dir = source.destination();
            let other_dir = if pane == self.active_pane {
                self.inactive.as_deref().and_then(App::destination)
            } else {
                self.destination()
            };
            let job = match command {
                Command::Copy(cut) => {
                    if paths.is_empty() {
                        self.file_message = "Select a filesystem file or folder first".into();
                    } else {
                        match crate::clipboard::write(&Files {
                            paths: paths.clone(),
                            cut,
                        }) {
                            Ok(()) => {
                                self.file_message = format!(
                                    "{} {} item(s)",
                                    if cut { "Cut" } else { "Copied" },
                                    paths.len()
                                );
                            }
                            Err(e) => self.file_message = e.to_string(),
                        }
                    }
                    None
                }
                Command::NewFolder => {
                    if let Some(dir)=dir {if pane!=self.active_pane{self.switch_pane(pane);}self.files.new_folder=Some((dir,"New folder".into()));}
                    else{self.file_message="Open a filesystem folder to create a folder".into();}
                    None
                }
                Command::Rename => {
                    if paths.len() == 1 {
                        let path = paths[0].clone();
                        let name = file_name(&path);
                        if pane == self.active_pane {
                            self.files.rename = Some((path, name));
                        } else {
                            self.switch_pane(pane);
                            self.files.rename = Some((path, name));
                        }
                    } else {
                        self.file_message = "Select exactly one file or folder to rename".into();
                    }
                    None
                }
                Command::Delete => {
                    if let Some(id)=&source.files.collection.clone(){self.collection_message(collections::remove(id,&paths));}else if !paths.is_empty() {
                        self.workbench.recycle = Some(paths);
                    }
                    None
                }
                Command::Duplicate => Some(Job::Duplicate(paths)),
                Command::Paste => match (dir, crate::clipboard::read()) {
                    (Some(dir), Ok(files)) => Some(if files.cut {
                        Job::Move(files.paths, dir)
                    } else {
                        Job::Copy(files.paths, dir)
                    }),
                    (None, _) => {
                        self.file_message = "Open a filesystem folder to paste".into();
                        None
                    }
                    (_, Err(e)) => {
                        self.file_message = e.to_string();
                        None
                    }
                },
                Command::Other(moving) => {
                    if self.dual_panes {
                        if let Some(dir) = other_dir {
                            Some(if moving {
                                Job::Move(paths, dir)
                            } else {
                                Job::Copy(paths, dir)
                            })
                        } else {
                            self.file_message = "Open a filesystem folder in the other pane".into();
                            None
                        }
                    } else {
                        self.file_message = "Enable Dual panes first".into();
                        None
                    }
                }
                Command::Drop(files, dir) => Some(if files.cut {
                    Job::Move(files.paths, dir)
                } else {
                    Job::Copy(files.paths, dir)
                }),
            };
            if let Some(job) = job {
                self.start_file_job(job, ctx);
            }
        }
    }
    fn release_file_handles(&mut self, sources: &[PathBuf]) {
        let affected = self
            .current
            .and_then(|i| self.images.get(i))
            .is_some_and(|image| match &image.src {
                ImageSrc::File(p) => sources
                    .iter()
                    .any(|source| p == source || p.starts_with(source)),
                ImageSrc::Entry { archive, .. } => sources
                    .iter()
                    .any(|source| archive == source || archive.starts_with(source)),
            });
        if affected {
            self.photos.cancel();
            self.video = None;
            self.scrubber = None;
            self.pdf_scrubber = None;
            self.pdf_request = None;
        }
        for tab in &mut self.tabs {
            if let Some(state) = tab.state.as_mut() {
                state.release_file_handles(sources);
            }
        }
        if let Some(other) = self.inactive.as_mut() {
            other.release_file_handles(sources);
        }
    }
    fn start_file_job(&mut self, job: Job, ctx: &egui::Context) {
        if matches!(&job,Job::Copy(paths,_)|Job::Move(paths,_)|Job::Duplicate(paths)|Job::Recycle(paths) if paths.is_empty())
        {
            self.file_message = "Select a filesystem file or folder first".into();
            return;
        }
        if self.file_job.is_some() {
            self.file_message =
                "A file operation is in progress; wait before starting another".into();
            return;
        }
        match &job {
            Job::Move(paths, _) | Job::Recycle(paths) => self.release_file_handles(paths),
            Job::Rename(path, _) => self.release_file_handles(std::slice::from_ref(path)),
            _ => {}
        }
        let (tx, rx) = mpsc::channel();
        self.file_job = Some(rx);
        self.file_message = "Working…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = crate::fileops::run(job);
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
    }
    fn refresh_after_files(&mut self, outcome: &crate::fileops::Outcome, ctx: &egui::Context) {
        if self.files.search.results {self.submit_search(ctx);}
        self.archive_cache.clear();
        if let Some(node) = self.selected.clone() {
            let changed = match &node {
                Node::Collection{..}=>true,
                Node::Dir(p) => {
                    outcome
                        .changed
                        .iter()
                        .any(|d| std::fs::canonicalize(d).ok() == std::fs::canonicalize(p).ok())
                        || !p.exists()
                }
                _ => false,
            };
            if changed && !self.files.search.results {
                let target = match node {
                    Node::Dir(mut p) => {
                        while !p.exists() {
                            if !p.pop() {
                                break;
                            }
                        }
                        if p.is_dir() {
                            Node::Dir(p)
                        } else {
                            Node::Computer
                        }
                    }
                    n => n,
                };
                self.select(target, ctx);
            }
        }
        for tab in &mut self.tabs {
            if let Some(state) = tab.state.as_mut() {
                state.refresh_after_files(outcome, ctx);
            }
        }
        if let Some(other) = self.inactive.as_mut() {
            other.refresh_after_files(outcome, ctx);
        }
    }
}

impl State {
    pub fn rows_for_media(&self, media: usize) -> Option<usize> {
        self.rows.iter().position(|row| row.media == Some(media))
    }
    pub fn select_only(&mut self, index: usize) {
        self.selected.clear();
        self.selected.insert(index);
        self.anchor = Some(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_commands_transfer_between_panes_and_refresh_tabs() {
        let root = std::env::temp_dir().join(format!(
            "view-calipers-panes-files-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let left = root.join("left");
        let right = root.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let source = left.join("unknown.xyz");
        std::fs::write(&source, b"arbitrary file").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.select(Node::Dir(left.clone()), &ctx);
        assert_eq!(app.files.rows.len(), 1);
        assert_eq!(app.files.rows[0].size, "14 B");
        assert!(app.files.rows[0].media.is_none());
        assert_ne!(app.files.rows[0].created, "—");
        app.files.details = true;
        app.files.select_only(0);
        app.set_dual(true, &ctx);
        app.inactive
            .as_mut()
            .unwrap()
            .select(Node::Dir(right.clone()), &ctx);
        app.new_tab(true, &ctx);
        assert!(app.files.details);
        app.files.select_only(0);
        app.files.pending = Some(Command::Other(true));
        app.file_commands(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.file_job.is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.file_commands(&ctx);
        }
        assert!(app.file_job.is_none());
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(right.join("unknown.xyz")).unwrap(),
            b"arbitrary file"
        );
        assert!(app.files.rows.is_empty());
        assert_eq!(app.inactive.as_ref().unwrap().files.rows.len(), 1);
        app.switch_tab(0);
        assert!(app.files.rows.is_empty());
        assert!(app.files.details);
        app.switch_pane(1);
        assert!(!app.files.details);
        assert_eq!(app.files.rows.len(), 1);
        assert_eq!(app.files.address, right.display().to_string());
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl App {
    pub fn select_all_files(&mut self) {
        self.files.selected = (0..self.files.rows.len()).collect();
    }
    pub fn file_counts(&self) -> (usize, usize) {
        (self.files.selected.len(), self.files.rows.len())
    }
}

pub(super) struct Marquee {
    start: Vec2,
    base: HashSet<usize>,
    additive: bool,
}
fn box_hits(
    rect: Rect,
    origin: Pos2,
    size: Vec2,
    stride: Vec2,
    columns: usize,
    count: usize,
) -> HashSet<usize> {
    (0..count)
        .filter(|index| {
            let min = origin
                + Vec2::new(
                    (index % columns) as f32 * stride.x,
                    (index / columns) as f32 * stride.y,
                );
            rect.intersects(Rect::from_min_size(min, size))
        })
        .collect()
}
impl App {
    pub fn folder_drop_target(&mut self, ui: &mut egui::Ui, response: &egui::Response, dir: &Path) {
        if let Some(payload) = egui::DragAndDrop::payload::<Files>(ui.ctx()) {
            if response.contains_pointer() {
                ui.painter().rect_stroke(
                    response.rect.shrink(1.0),
                    4.0,
                    egui::Stroke::new(2.0_f32, crate::ui_theme::ACCENT),
                );
                response.clone().on_hover_text(format!(
                    "{} {} item(s) to {} · Ctrl to copy",
                    if ui.input(|i| i.modifiers.ctrl) {
                        "Copy"
                    } else {
                        "Move"
                    },
                    payload.paths.len(),
                    dir.display()
                ));
            }
            if let Some(payload) = response.dnd_release_payload::<Files>() {
                let mut files = (*payload).clone();
                files.cut = !ui.input(|i| i.modifiers.ctrl);
                self.files.pending = Some(Command::Drop(files, dir.into()));
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn marquee_selection(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        origin: Pos2,
        size: Vec2,
        stride: Vec2,
        columns: usize,
        count: usize,
        visible: &[Rect],
    ) {
        let (pressed, down, released, position, modifiers, escape) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.modifiers,
                i.key_pressed(Key::Escape),
            )
        });
        if escape {
            if let Some(m) = self.files.marquee.take() {
                self.files.selected = m.base;
            }
            return;
        }
        if pressed && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            if let Some(pos) = position.filter(|pos| {
                area.contains(*pos)
                    && !visible.iter().any(|r| r.contains(*pos))
                    && (!self.files.details || pos.y >= origin.y)
            }) {
                self.files.marquee = Some(Marquee {
                    start: pos - origin,
                    base: self.files.selected.clone(),
                    additive: modifiers.ctrl || modifiers.shift,
                });
                if !modifiers.ctrl && !modifiers.shift {
                    self.files.selected.clear();
                }
            }
        }
        if let (Some(m), Some(pos)) = (self.files.marquee.as_ref(), position) {
            let start = origin + m.start;
            if start.distance(pos) > 3.0 {
                let bounds = Rect::from_two_pos(start, pos);
                let hits = box_hits(bounds, origin, size, stride, columns, count);
                self.files.selected = if m.additive {
                    m.base.union(&hits).copied().collect()
                } else {
                    hits
                };
                self.files.anchor = self.files.selected.iter().copied().min();
                let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
                painter.rect_filled(bounds, 0.0, crate::ui_theme::ACCENT.gamma_multiply(0.18));
                painter.rect_stroke(
                    bounds,
                    0.0,
                    egui::Stroke::new(1.0_f32, crate::ui_theme::ACCENT),
                );
                ui.ctx().request_repaint();
            }
        }
        if released || !down {
            self.files.marquee = None;
        }
    }
}
#[cfg(test)]
mod marquee_tests {
    use super::*;
    #[test]
    fn box_selects_grid_and_details_in_both_directions_with_scrolled_origin() {
        let origin = Pos2::new(10.0, -190.0);
        let size = Vec2::new(100.0, 80.0);
        let stride = Vec2::new(110.0, 90.0);
        let bounds = Rect::from_two_pos(Pos2::new(215.0, 65.0), Pos2::new(15.0, 0.0));
        assert_eq!(
            box_hits(bounds, origin, size, stride, 3, 12),
            HashSet::from([6, 7])
        );
        assert_eq!(
            box_hits(
                Rect::from_min_max(Pos2::new(300.0, 1.0), Pos2::new(340.0, 50.0)),
                origin,
                Vec2::new(400.0, 26.0),
                Vec2::new(406.0, 30.0),
                1,
                30
            ),
            HashSet::from([6, 7, 8])
        );
        assert!(box_hits(
            Rect::from_min_size(Pos2::new(111.0, -188.0), Vec2::splat(2.0)),
            origin,
            size,
            stride,
            3,
            12
        )
        .is_empty());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    fn input(events: Vec<egui::Event>, modifiers: egui::Modifiers) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            modifiers,
            ..Default::default()
        }
    }
    fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }
    #[test]
    fn rubber_band_tracks_drag_adds_with_ctrl_and_restores_on_escape() {
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        let origin = Pos2::new(20.0, 20.0);
        let area = Rect::from_min_size(origin, Vec2::new(500.0, 400.0));
        let size = Vec2::new(100.0, 80.0);
        let stride = Vec2::new(110.0, 90.0);
        let visible = [Rect::from_min_size(origin, size)];
        let mut frame = |events, modifiers| {
            let _ = ctx.run(input(events, modifiers), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.marquee_selection(ui, area, origin, size, stride, 3, 6, &visible)
                });
            });
        };
        let start = Pos2::new(355.0, 205.0);
        let end = Pos2::new(25.0, 25.0);
        frame(
            vec![
                egui::Event::PointerMoved(start),
                button(start, true, Default::default()),
            ],
            Default::default(),
        );
        frame(vec![egui::Event::PointerMoved(end)], Default::default());
        frame(
            vec![button(end, false, Default::default())],
            Default::default(),
        );
        assert_eq!(app.files.selected, HashSet::from([0, 1, 2, 3, 4, 5]));
        assert!(app.files.marquee.is_none());
        app.files.selected = HashSet::from([5]);
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let start = Pos2::new(125.0, 30.0);
        let end = Pos2::new(25.0, 90.0);
        for (events, modifiers) in [
            (
                vec![egui::Event::PointerMoved(start), button(start, true, ctrl)],
                ctrl,
            ),
            (vec![egui::Event::PointerMoved(end)], ctrl),
        ] {
            let _ = ctx.run(input(events, modifiers), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.marquee_selection(ui, area, origin, size, stride, 3, 6, &visible)
                });
            });
        }
        assert_eq!(app.files.selected, HashSet::from([0, 5]));
        let _ = ctx.run(
            input(
                vec![egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: ctrl,
                }],
                ctrl,
            ),
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.marquee_selection(ui, area, origin, size, stride, 3, 6, &visible)
                });
            },
        );
        assert_eq!(app.files.selected, HashSet::from([5]));
        assert!(app.files.marquee.is_none());
    }
    #[test]
    fn folder_tree_accepts_group_drag_and_ctrl_changes_move_to_copy() {
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        let dir = std::env::temp_dir().join("view-calipers-tree-drop-target");
        let node = Node::Dir(dir.clone());
        let point = Pos2::new(85.0, 18.0);
        let paths = vec![dir.join("a.txt"), dir.join("b.txt")];
        for ctrl in [false, true] {
            let modifiers = egui::Modifiers {
                ctrl,
                ..Default::default()
            };
            let _ = ctx.run(
                input(
                    vec![
                        egui::Event::PointerMoved(point),
                        button(point, true, modifiers),
                    ],
                    modifiers,
                ),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.show_tree(ui, &node, 0);
                    });
                },
            );
            egui::DragAndDrop::set_payload(
                &ctx,
                Files {
                    paths: paths.clone(),
                    cut: true,
                },
            );
            let _ = ctx.run(
                input(vec![button(point, false, modifiers)], modifiers),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.show_tree(ui, &node, 0);
                    });
                },
            );
            match app.files.pending.take() {
                Some(Command::Drop(files, target)) => {
                    assert_eq!(target, dir);
                    assert_eq!(files.paths, paths);
                    assert_eq!(files.cut, !ctrl);
                }
                _ => panic!("Tree did not accept file drag"),
            }
        }
    }
}

impl App {
    pub(super) fn install_search_results(&mut self,hits:Vec<everything::Hit>,ctx:&egui::Context){
        if !self.close_board(){return;}
        self.files.collection=None;self.files.collection_order.clear();
        self.photos.cancel();self.photo_last=None;self.current=None;self.texture=None;self.video=None;self.scrubber=None;self.pdf=None;self.pdf_scrubber=None;self.pdf_request=None;self.model=None;self.model_request=None;self.markdown=None;
        self.images.clear();self.gallery_nodes.clear();let mut rows=Vec::new();
        for hit in hits {let ext=ext_of_path(&hit.path);let node=if hit.folder{Some(Node::Dir(hit.path.clone()))}else if is_archive_ext(&ext){Some(Node::Archive(hit.path.clone()))}else{None};
            let (media,thumb)=if let Some(node)=&node{let index=self.gallery_nodes.len();self.gallery_nodes.push(node.clone());(None,Some(index))}
                else if is_image_ext(&ext)||VIDEO_EXT.contains(&ext.as_str())||DOCUMENT_EXT.contains(&ext.as_str())||model3d::EXTENSIONS.contains(&ext.as_str()) {let index=self.images.len();self.images.push(ImageRef{label:file_name(&hit.path),src:ImageSrc::File(hit.path.clone())});(Some(index),Some(index))}else{(None,None)};
            rows.push(Row{label:file_name(&hit.path),path:Some(hit.path),node,media,thumb,bytes:hit.bytes,size:hit.bytes.map(size).unwrap_or_else(||"—".into()),created:hit.created,modified:hit.modified});
        }
        for row in &mut rows{if row.node.is_some(){if let Some(index)=row.thumb.as_mut(){*index+=self.images.len();}}}
        self.files.rows=rows;self.files.all_rows=self.files.rows.clone();self.files.selected.clear();self.files.anchor=None;self.files.marquee=None;self.files.search.results=true;self.files.sort_rows();self.gallery=true;
        self.start_thumbnails(ctx);
    }
}

#[cfg(test)]
mod sort_tests {
    use super::*;
    fn row(name:&str,bytes:u64,media:usize)->Row{Row{label:name.into(),path:Some(PathBuf::from(name)),node:None,media:Some(media),thumb:Some(media),bytes:Some(bytes),size:size(bytes),created:format!("2026-10-{:02} 00:00:00",media+1),modified:format!("2026-10-{:02} 00:00:00",media+2)}}
    #[test]
    fn sorting_preserves_selection_and_thumbnail_identity(){
        let mut state=State::default();state.rows=vec![row("image10.jpg",10,0),row("image2.png",2000,1),row("image1.jpg",100,2)];
        state.select_only(0);state.sort_rows();assert_eq!(state.rows.iter().map(|r|r.label.as_str()).collect::<Vec<_>>(),vec!["image1.jpg","image2.png","image10.jpg"]);assert_eq!(state.anchor,Some(2));assert!(state.selected.contains(&2));
        state.sort=Sort{key:SortKey::Size,descending:true};state.sort_rows();assert_eq!(state.rows.iter().map(|r|r.media.unwrap()).collect::<Vec<_>>(),vec![1,2,0]);assert_eq!(state.rows[0].thumb,Some(1));assert!(state.selected.contains(&2));
        state.details=true;state.details=false;state.sort_rows();assert_eq!(state.rows[0].bytes,Some(2000));
        state.sort=Sort{key:SortKey::Created,descending:false};state.sort_rows();assert_eq!(state.rows[0].media,Some(0));
        state.sort=Sort{key:SortKey::Modified,descending:true};state.sort_rows();assert_eq!(state.rows[0].media,Some(2));
        state.sort=Sort{key:SortKey::Type,descending:true};state.sort_rows();assert_eq!(state.rows[0].label,"image2.png");
    }
    #[test]
    fn folders_stay_first_and_navigation_follows_sorted_rows(){
        let mut app=App::new(None);let mut folder=row("z",0,0);folder.node=Some(Node::Dir(PathBuf::from("z")));folder.media=None;folder.bytes=None;
        app.files.rows=vec![row("a.jpg",1,0),folder,row("b.jpg",2,1)];app.files.all_rows=app.files.rows.clone();app.files.sort=Sort{key:SortKey::Name,descending:true};app.files.sort_rows();assert!(app.files.rows[0].folder());assert_eq!(app.media_navigation(),vec![1,0]);
        app.files.search.query="jpg".into();app.apply_file_filter();assert_eq!(app.media_navigation(),vec![1,0]);
    }
    #[test]
    fn new_folder_does_not_overwrite_or_escape_parent(){
        let root=std::env::temp_dir().join(format!("view-calipers-new-folder-{}",std::process::id()));std::fs::create_dir_all(&root).unwrap();
        let result=crate::fileops::run(Job::NewFolder(root.clone(),"Test folder".into()));assert!(result.errors.is_empty());assert!(result.paths[0].is_dir());assert_eq!(result.changed,vec![root.clone()]);
        assert!(!crate::fileops::run(Job::NewFolder(root.clone(),"Test folder".into())).errors.is_empty());
        for invalid in ["../escape","CON","trailing.",""]{assert!(!crate::fileops::run(Job::NewFolder(root.clone(),invalid.into())).errors.is_empty());}
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl App {
    fn folder_view(&self)->crate::folder_views::View{crate::folder_views::View{details:self.files.details,sort:self.files.sort,thumb:self.thumb_size,layout:self.files.columns.clone()}}
    pub(super) fn save_folder_view(&mut self,force:bool){
        if self.files.search.results {return;}
        let Some(folder)=self.files.view_folder.clone()else{return;};
        let view=self.folder_view();if self.files.view_saved.as_ref()==Some(&view){self.files.view_pending=None;return;}
        if self.files.view_pending.as_ref().is_none_or(|(pending,_)|pending!=&view){self.files.view_pending=Some((view.clone(),std::time::Instant::now()));}
        if !force && self.files.view_pending.as_ref().is_some_and(|(_,time)|time.elapsed()<std::time::Duration::from_millis(500)){return;}
        match crate::folder_views::save(&folder,&view){Ok(())=>{self.files.view_saved=Some(view);self.files.view_pending=None;},Err(error)=>{self.file_message=format!("Could not save folder view: {error}");}}
    }
    pub(super) fn restore_folder_view(&mut self){
        let folder=match &self.selected{Some(Node::Dir(folder))=>Some(folder.clone()),_=>None};
        if self.files.view_folder==folder{return;}
        self.files.view_folder=folder.clone();self.files.view_pending=None;
        let saved=folder.as_deref().and_then(crate::folder_views::load);
        self.files.view_saved=saved.clone();
        if let Some(view)=saved{self.files.details=view.details;self.files.sort=view.sort;self.thumb_size=view.thumb;self.files.columns=view.layout;}
    }
}

#[cfg(test)]
mod folder_view_interaction_tests {
    use super::*;
    fn frame(app:&mut App,ctx:&egui::Context,events:Vec<egui::Event>)->egui::FullOutput{
        ctx.run(egui::RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,Vec2::new(1100.0,800.0))),events,..Default::default()},|ctx|{egui::CentralPanel::default().show(ctx,|ui|app.file_gallery(ui));})
    }
    fn point(output:&egui::FullOutput,label:&str)->Pos2{output.shapes.iter().find_map(|s|if let egui::epaint::Shape::Text(text)=&s.shape{(text.galley.job.text==label).then(||text.pos+text.galley.size()*0.5)}else{None}).unwrap_or_else(||panic!("Missing header {label}"))}
    fn button(pos:Pos2,pressed:bool)->egui::Event{egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE}}
    fn click(app:&mut App,ctx:&egui::Context,pos:Pos2){frame(app,ctx,vec![egui::Event::PointerMoved(pos),button(pos,true)]);frame(app,ctx,vec![button(pos,false)]);}
    #[test]
    fn headers_sort_resize_and_drag_without_changing_file_selection(){
        let ctx=egui::Context::default();let mut app=App::new(None);app.files.details=true;
        let output=frame(&mut app,&ctx,vec![]);let name=point(&output,"Name ↑");click(&mut app,&ctx,name);assert!(app.files.sort.descending);
        let output=frame(&mut app,&ctx,vec![]);let name=point(&output,"Name ↓");
        let galley=output.shapes.iter().find_map(|s|if let egui::epaint::Shape::Text(t)=&s.shape{(t.galley.job.text=="Name ↓").then_some(t.pos)}else{None}).unwrap();
        let edge=Pos2::new(galley.x-6.0+270.0,name.y);
        frame(&mut app,&ctx,vec![egui::Event::PointerMoved(edge),button(edge,true)]);
        frame(&mut app,&ctx,vec![egui::Event::PointerMoved(edge+Vec2::new(80.0,0.0))]);
        frame(&mut app,&ctx,vec![button(edge+Vec2::new(80.0,0.0),false)]);
        assert_eq!(app.files.columns.columns[0].width,350.0);
        let output=frame(&mut app,&ctx,vec![]);let name=point(&output,"Name ↓");let modified=point(&output,"Modified");
        frame(&mut app,&ctx,vec![egui::Event::PointerMoved(name),button(name,true)]);
        frame(&mut app,&ctx,vec![egui::Event::PointerMoved(name+Vec2::new(20.0,0.0))]);
        frame(&mut app,&ctx,vec![egui::Event::PointerMoved(modified)]);
        frame(&mut app,&ctx,vec![button(modified,false)]);
        assert!(app.files.columns.columns[4].key==SortKey::Name);assert!(app.files.sort.descending);
    }
    #[test]
    fn revisiting_folder_and_restart_restore_view_while_search_keeps_original(){
        let ctx=egui::Context::default();let root=std::env::temp_dir().join(format!("view-calipers-folder-restore-{}",std::process::id()));let a=root.join("a");let b=root.join("b");std::fs::create_dir_all(&a).unwrap();std::fs::create_dir_all(&b).unwrap();
        let mut app=App::new(None);app.select(Node::Dir(a.clone()),&ctx);app.files.details=true;app.files.sort=Sort{key:SortKey::Size,descending:true};app.thumb_size=280.0;app.files.columns.resize(SortKey::Name,50.0);
        app.select(Node::Dir(b.clone()),&ctx);app.files.details=false;app.thumb_size=100.0;app.select(Node::Dir(a.clone()),&ctx);assert!(app.files.details);assert_eq!(app.thumb_size,280.0);assert!(app.files.sort.key==SortKey::Size);assert_eq!(app.files.columns.columns[0].width,320.0);
        app.files.search.results=true;app.thumb_size=80.0;app.files.details=false;app.save_folder_view(true);app.select(Node::Dir(a.clone()),&ctx);assert!(app.files.details);assert_eq!(app.thumb_size,280.0);
        let mut restarted=App::new(None);restarted.select(Node::Dir(a),&ctx);assert!(restarted.files.details);assert_eq!(restarted.thumb_size,280.0);
        restarted.select(Node::Dir(b),&ctx);assert!(!restarted.files.details);assert_eq!(restarted.thumb_size,100.0);std::fs::remove_dir_all(root).unwrap();
    }
}

impl App {
    pub(super) fn reveal_original(&mut self,path:&Path,ctx:&egui::Context){
        if let Some(parent)=path.parent().filter(|p|p.is_dir()){
            self.select(Node::Dir(parent.into()),ctx);
            if let Some(row)=self.files.rows.iter().position(|r|r.path.as_deref()==Some(path)){
                self.files.select_only(row);
                if let Some(media)=self.files.rows[row].media{self.goto(media,ctx);}
            }else{self.file_message="The original file is unavailable".into();}
        }else{self.file_message="The original folder is unavailable".into();}
    }
}
