use super::*;
use crate::file_browser::Row;
#[derive(Default)]
pub struct State {
    pub query: String,
    pub scope: usize,
    pub kind: usize,
    pub regex: bool,
    pub focus: bool,
    pub results: bool,
    pub offset: usize,
    pub total: usize,
    pub message: String,
    pub due: Option<std::time::Instant>,
    pub client: everything::Client,
}
impl State {
    pub fn navigation_reset(&mut self) {
        self.client.cancel();
        self.due = None;
        if self.results || self.scope != 0 {
            self.query.clear();
            self.scope = 0;
            self.kind = 0;
            self.regex = false;
        }
        self.results = false;
        self.offset = 0;
        self.total = 0;
        self.message.clear();
    }
}
const KINDS: [&str; 7] = [
    "All types",
    "Images",
    "Video",
    "Documents",
    "3D",
    "Archives",
    "Folders",
];
fn kind_matches(kind: usize, folder: bool, ext: &str) -> bool {
    match kind {
        0 => true,
        1 => is_image_ext(ext),
        2 => VIDEO_EXT.contains(&ext),
        3 => DOCUMENT_EXT.contains(&ext),
        4 => model3d::EXTENSIONS.contains(&ext),
        5 => is_archive_ext(ext),
        6 => folder,
        _ => false,
    }
}
fn kind_query(kind: usize) -> String {
    let extensions = match kind {
        1 => IMAGE_EXT,
        2 => VIDEO_EXT,
        3 => DOCUMENT_EXT,
        4 => model3d::EXTENSIONS,
        5 => ARCHIVE_EXT,
        6 => return "folder:".into(),
        _ => return String::new(),
    };
    format!("ext:{}", extensions.join(";"))
}
fn global_query(query: &str, kind: usize, scope: Option<&Path>) -> String {
    let mut terms = vec![kind_query(kind)];
    if !query.trim().is_empty() {
        terms.push(format!("<{}>", query.trim()));
    }
    if let Some(path) = scope {
        terms.push(format!(
            "path:\"{}\\\"",
            path.display().to_string().trim_end_matches(['\\', '/'])
        ));
    }
    terms.join(" ")
}
#[derive(Clone, Copy)]
enum Compare {
    Less,
    LessEqual,
    Equal,
    GreaterEqual,
    Greater,
}
impl Compare {
    fn apply<T: Ord>(&self, a: T, b: T) -> bool {
        match self {
            Self::Less => a < b,
            Self::LessEqual => a <= b,
            Self::Equal => a == b,
            Self::GreaterEqual => a >= b,
            Self::Greater => a > b,
        }
    }
}
fn comparison(value: &str) -> (Compare, &str) {
    for (op, cmp) in [
        ("<=", Compare::LessEqual),
        (">=", Compare::GreaterEqual),
        ("<", Compare::Less),
        (">", Compare::Greater),
        ("=", Compare::Equal),
    ] {
        if let Some(value) = value.strip_prefix(op) {
            return (cmp, value);
        }
    }
    (Compare::Equal, value)
}
fn bytes(value: &str) -> std::result::Result<u64, String> {
    let value = value.to_ascii_lowercase();
    let n = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let number = value[..n]
        .parse::<f64>()
        .map_err(|_| "Invalid size".to_string())?;
    let multiplier = match value[n..].trim() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1048576.0,
        "g" | "gb" | "gib" => 1073741824.0,
        _ => return Err("Use B, KB, MB or GB for size".into()),
    };
    let result = number * multiplier;
    if !result.is_finite() || result < 0.0 || result >= u64::MAX as f64 {
        return Err("Size is out of range".into());
    }
    Ok(result as u64)
}
enum Predicate {
    Text(regex::Regex, bool),
    Ext(Vec<String>),
    Size(Compare, u64),
    Date(bool, Compare, String),
    Folder(bool),
}
struct Term {
    predicate: Predicate,
    negative: bool,
}
pub struct Filter {
    groups: Vec<Vec<Term>>,
}
impl Filter {
    fn parse(query: &str, regex: bool) -> std::result::Result<Self, String> {
        if regex {
            let pattern = regex::RegexBuilder::new(query)
                .case_insensitive(true)
                .build()
                .map_err(|e| format!("Invalid regex: {e}"))?;
            return Ok(Self {
                groups: vec![vec![Term {
                    predicate: Predicate::Text(pattern, false),
                    negative: false,
                }]],
            });
        }
        let mut tokens = Vec::new();
        let mut token = String::new();
        let mut quoted = false;
        for c in query.chars() {
            if c == '"' {
                quoted = !quoted;
            } else if !quoted && (c.is_whitespace() || c == '|') {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                if c == '|' {
                    tokens.push("|".into());
                }
            } else {
                token.push(c);
            }
        }
        if quoted {
            return Err("Close the quoted phrase".into());
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        let mut groups = vec![Vec::new()];
        for token in tokens {
            if token == "|" {
                if groups.last().unwrap().is_empty() {
                    return Err("Add a term before OR".into());
                }
                groups.push(Vec::new());
                continue;
            }
            let negative = token.starts_with('!');
            let token = token.strip_prefix('!').unwrap_or(&token);
            let lower = token.to_lowercase();
            let predicate = if let Some(value) = lower.strip_prefix("ext:") {
                Predicate::Ext(
                    value
                        .split(';')
                        .map(|e| e.trim_start_matches('.').to_string())
                        .collect(),
                )
            } else if let Some(value) = lower.strip_prefix("size:") {
                let (cmp, value) = comparison(value);
                Predicate::Size(cmp, bytes(value)?)
            } else if lower.starts_with("dc:") || lower.starts_with("dm:") {
                let (cmp, value) = comparison(&lower[3..]);
                if value.len() != 10
                    || value.chars().enumerate().any(|(i, c)| {
                        if i == 4 || i == 7 {
                            c != '-'
                        } else {
                            !c.is_ascii_digit()
                        }
                    })
                {
                    return Err("Use YYYY-MM-DD for dates".into());
                }
                Predicate::Date(lower.starts_with("dm:"), cmp, value.into())
            } else if lower == "folder:" {
                Predicate::Folder(true)
            } else if lower == "file:" {
                Predicate::Folder(false)
            } else {
                let (value, path, regular) = if let Some(v) = token.strip_prefix("path:") {
                    (v, true, false)
                } else if let Some(v) = token.strip_prefix("regex:") {
                    (v, false, true)
                } else if let Some(v) = token.strip_prefix("name:") {
                    (v, false, false)
                } else {
                    (token, false, false)
                };
                let pattern = if regular {
                    value.to_string()
                } else if value.contains(['*', '?']) {
                    format!(
                        "^{}$",
                        regex::escape(value)
                            .replace("\\*", ".*")
                            .replace("\\?", ".")
                    )
                } else {
                    regex::escape(value)
                };
                Predicate::Text(
                    regex::RegexBuilder::new(&pattern)
                        .case_insensitive(true)
                        .build()
                        .map_err(|e| format!("Invalid regex: {e}"))?,
                    path,
                )
            };
            groups.last_mut().unwrap().push(Term {
                predicate,
                negative,
            });
        }
        if groups.len() > 1 && groups.last().unwrap().is_empty() {
            return Err("Add a term after OR".into());
        }
        Ok(Self { groups })
    }
    fn matches(&self, row: &Row) -> bool {
        let folder = row
            .node
            .as_ref()
            .is_some_and(|node| matches!(node, Node::Dir(_) | Node::Computer));
        let ext = row.extension();
        self.groups.iter().any(|group| {
            group.iter().all(|term| {
                let yes = match &term.predicate {
                    Predicate::Text(regex, path) => {
                        if *path {
                            regex.is_match(
                                &row.path
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| row.label.clone()),
                            )
                        } else {
                            regex.is_match(&row.label)
                        }
                    }
                    Predicate::Ext(types) => types.contains(&ext),
                    Predicate::Size(cmp, value) => {
                        row.bytes.is_some_and(|bytes| cmp.apply(bytes, *value))
                    }
                    Predicate::Date(modified, cmp, value) => {
                        let date = if *modified {
                            &row.modified
                        } else {
                            &row.created
                        };
                        date.get(..10)
                            .is_some_and(|date| cmp.apply(date, value.as_str()))
                    }
                    Predicate::Folder(value) => folder == *value,
                };
                yes != term.negative
            })
        })
    }
}
#[derive(Clone)]
struct Saved {
    scope: usize,
    kind: usize,
    regex: bool,
    query: String,
}
fn saved_path() -> PathBuf {
    settings::Settings::path().with_file_name("searches.tsv")
}
fn read_saved() -> Vec<Saved> {
    std::fs::read_to_string(saved_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let values: Vec<_> = line.splitn(4, '\t').collect();
            if values.len() != 4 {
                return None;
            }
            Some(Saved {
                scope: values[0].parse::<usize>().ok()?.min(2),
                kind: values[1].parse::<usize>().ok()?.min(6),
                regex: values[2] == "1",
                query: values[3].into(),
            })
        })
        .collect()
}
fn save_search(state: &State) -> Result<()> {
    let mut saved = read_saved();
    saved.retain(|item| {
        item.query != state.query
            || item.scope != state.scope
            || item.kind != state.kind
            || item.regex != state.regex
    });
    saved.insert(
        0,
        Saved {
            scope: state.scope,
            kind: state.kind,
            regex: state.regex,
            query: state.query.replace(['\n', '\r', '\t'], " "),
        },
    );
    saved.truncate(20);
    let text = saved
        .iter()
        .map(|item| {
            format!(
                "{}\t{}\t{}\t{}",
                item.scope,
                item.kind,
                u8::from(item.regex),
                item.query
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let path = saved_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, text)?;
    Ok(())
}
impl App {
    pub(super) fn apply_file_filter(&mut self) {
        let s = &self.files.search;
        let filter = match Filter::parse(&s.query, s.regex) {
            Ok(filter) => filter,
            Err(error) => {
                self.files.search.message = error;
                return;
            }
        };
        let mut rows = Vec::new();
        for row in &self.files.all_rows {
            let folder = row
                .node
                .as_ref()
                .is_some_and(|n| matches!(n, Node::Dir(_) | Node::Computer));
            if row.extension()!="vcboard" && kind_matches(s.kind, folder, &row.extension()) && filter.matches(row) {
                rows.push(row.clone());
            }
        }
        self.files.rows = rows;
        self.files.sort_rows();
        self.files.selected.clear();
        self.files.anchor = None;
        self.files.marquee = None;
        self.files.search.message.clear();
    }
    pub(super) fn media_navigation(&self) -> Vec<usize> {
        if !self.files.all_rows.is_empty() || !self.files.search.query.is_empty() || self.files.search.kind != 0
        {
            self.files.rows.iter().filter_map(|row| row.media).collect()
        } else {
            (0..self.images.len()).collect()
        }
    }
    pub(super) fn submit_search(&mut self, ctx: &egui::Context) {
        let s = &mut self.files.search;
        if s.scope == 0 {
            return;
        }
        let folder = if s.scope == 2 {
            match &self.selected {
                Some(Node::Dir(path)) => Some(path.clone()),
                _ => {
                    s.message = "Choose a folder for recursive Everything search".into();
                    return;
                }
            }
        } else {
            None
        };
        let query = global_query(&s.query, s.kind, folder.as_deref());
        s.client.search_sorted(query, s.offset, self.files.sort.sdk(), ctx);
        s.due = None;
        s.message = "Searching Everything…".into();
    }
    pub(super) fn poll_search(&mut self, ctx: &egui::Context) {
        let due = self.files.search.due;
        if due.is_some_and(|due| due.elapsed() >= std::time::Duration::from_millis(220)) {
            self.submit_search(ctx);
        } else if due.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if let Some(result) = self.files.search.client.poll() {
            match result {
                Ok(result) => {
                    self.install_search_results(result.hits, ctx);
                    self.files.search.total = result.total;
                    self.files.search.message = format!(
                        "Everything · {}–{} / {} · {:.0} ms",
                        if result.total == 0 {
                            0
                        } else {
                            self.files.search.offset + 1
                        },
                        self.files.search.offset + self.files.rows.len(),
                        result.total,
                        result.elapsed.as_secs_f64() * 1000.0
                    );
                }
                Err(error) => self.files.search.message = error,
            }
        }
    }
    fn exit_search_results(&mut self, ctx: &egui::Context) {
        let node = self.selected.clone().unwrap_or(Node::Computer);
        self.files.view_folder=None;
        self.restore_folder_view();
        self.files.search.navigation_reset();
        self.files.search.scope = 0;
        self.files.search.query.clear();
        self.files.search.kind = 0;
        self.select(node, ctx);
    }
    pub(super) fn search_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let previous = self.files.search.scope;
        let mut changed = false;
        let mut submit = false;
        let mut exit = false;
        use crate::ui_theme::{self as theme,Icon};
        let wide=ui.available_width()>=650.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x=4.0;
            if wide {egui::ComboBox::from_id_salt("search_scope").selected_text(["Filter","Everything","Subfolders"][self.files.search.scope]).width(95.0).show_ui(ui,|ui|{
                for (i,label) in ["Filter this panel","Everything · all drives","Everything · folder + subfolders"].iter().enumerate(){changed|=ui.selectable_value(&mut self.files.search.scope,i,*label).changed();}
            });}
            let side=ui.spacing().interact_size.y;
            let reserve=side*2.0+12.0+if wide{120.0}else{0.0}+if self.files.search.scope!=0{side+4.0}else{0.0};
            let input=ui.add_sized([ (ui.available_width()-reserve).max(45.0),ui.spacing().interact_size.y ],egui::TextEdit::singleline(&mut self.files.search.query).hint_text(if self.files.search.scope==0{"Filter files · Ctrl+F"}else{"Search Everything · Enter"}));
            if self.files.search.focus {input.request_focus();self.files.search.focus=false;}
            changed|=input.changed();submit=input.has_focus()&&ui.input(|i|i.key_pressed(Key::Enter));
            if wide {egui::ComboBox::from_id_salt("search_kind").selected_text(KINDS[self.files.search.kind]).width(85.0).show_ui(ui,|ui|{for (i,label) in KINDS.iter().enumerate(){changed|=ui.selectable_value(&mut self.files.search.kind,i,*label).changed();}});}
            let active=self.files.search.kind!=0||self.files.search.regex||self.files.search.scope!=0;
            theme::icon_menu(ui,Icon::Filter,"Search options · type, scope, regex and saved searches",active,|ui|{
                ui.strong("Search scope");
                for (i,label) in ["Filter this panel","Everything · all drives","Everything · folder + subfolders"].iter().enumerate(){changed|=ui.selectable_value(&mut self.files.search.scope,i,*label).changed();}
                ui.separator();ui.strong("File type");
                egui::ComboBox::from_id_salt("filter_kind_menu").selected_text(KINDS[self.files.search.kind]).show_ui(ui,|ui|{for (i,label) in KINDS.iter().enumerate(){changed|=ui.selectable_value(&mut self.files.search.kind,i,*label).changed();}});
                if self.files.search.scope==0 {changed|=ui.checkbox(&mut self.files.search.regex,"Regex").changed();}
                ui.separator();ui.menu_button("Saved searches",|ui|{
                    if ui.button("Save current search").clicked(){self.files.search.message=match save_search(&self.files.search){Ok(())=>"Search saved".into(),Err(e)=>e.to_string()};ui.close_menu();}
                    ui.separator();for saved in read_saved(){if ui.button(&saved.query).clicked(){self.files.search.scope=saved.scope;self.files.search.kind=saved.kind;self.files.search.regex=saved.regex;self.files.search.query=saved.query;changed=true;submit=self.files.search.scope!=0;ui.close_menu();}}
                });
                ui.small("Query syntax · hover for help").on_hover_text("Local: words, quoted phrases, *, ?, !exclude, OR |, ext:jpg;png, size:>10mb, dc:>=2026-01-01, dm:<2026-10-01, regex:pattern. Everything accepts its native query syntax.");
                if self.files.search.results {ui.separator();if ui.button("Return to folder").clicked(){exit=true;ui.close_menu();}
                    if ui.add_enabled(self.files.search.offset>0,egui::Button::new("Previous 200 results")).clicked(){self.files.search.offset=self.files.search.offset.saturating_sub(200);submit=true;}
                    if ui.add_enabled(self.files.search.offset+200<self.files.search.total,egui::Button::new("Next 200 results")).clicked(){self.files.search.offset+=200;submit=true;}
                }
            });
            if self.files.search.scope!=0 && theme::button(ui,Icon::Search,"Search Everything",true,false).clicked(){submit=true;}
            if theme::button(ui,Icon::Close,"Clear search / return to folder",!self.files.search.query.is_empty()||self.files.search.results,false).clicked(){
                if self.files.search.results {exit=true;}else{self.files.search.query.clear();changed=true;}
            }
        });
        if exit { self.exit_search_results(&ctx); return; }
        if previous != 0 && self.files.search.scope == 0 && self.files.search.results {
            let query = self.files.search.query.clone();
            let kind = self.files.search.kind;
            let regex = self.files.search.regex;
            self.exit_search_results(&ctx);
            self.files.search.query = query;
            self.files.search.kind = kind;
            self.files.search.regex = regex;
            self.apply_file_filter();
            return;
        }
        if changed {
            self.files.search.client.cancel();
            self.files.search.due = None;
            self.files.search.offset = 0;
            if self.files.search.scope == 0 {
                self.apply_file_filter();
            } else if !self.files.search.query.trim().is_empty() || self.files.search.kind != 0 {
                self.files.search.due = Some(std::time::Instant::now());
                ctx.request_repaint();
            }
        }
        if submit {
            if self.files.search.scope == 0 {
                self.apply_file_filter();
            } else {
                self.submit_search(&ctx);
            }
        }
        if self.files.search.client.busy {
            ui.spinner();
        }
        if !self.files.search.message.is_empty() {
            ui.add(egui::Label::new(RichText::new(&self.files.search.message).small()).truncate())
                .on_hover_text(&self.files.search.message);
        }
        if self.files.search.scope != 0 && self.files.search.message.contains("not running") {
            if ui.small_button("Start Everything").clicked() {
                self.files.search.message = match everything::start() {
                    Ok(()) => "Starting Everything. Press Search when its index is ready.".into(),
                    Err(error) => error.to_string(),
                };
            }
        }
        ui.separator();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(name: &str, bytes: u64) -> Row {
        Row { label:name.into(),path:Some(PathBuf::from(format!("C:/media/{name}"))),node:None,media:Some(7),thumb:Some(7),size:String::new(),bytes:Some(bytes),created:"2026-10-01 12:00:00".into(),modified:"2026-10-09 12:00:00".into() }
    }
    #[test] fn compound_filters_and_unicode() {
        let r=row("Сессия test 01.JPG",2*1024*1024);
        assert!(Filter::parse("ext:jpg;png size:>=2MB dc:>=2026-10-01 dm:<2026-10-10",false).unwrap().matches(&r));
        assert!(Filter::parse("\"Сессия test\" !draft | other",false).unwrap().matches(&r));
        assert!(Filter::parse("*.jpg",false).unwrap().matches(&r));
        assert!(!Filter::parse("*.png | size:>3MB",false).unwrap().matches(&r));
        assert!(Filter::parse("path:media regex:01\\.JPG$",false).unwrap().matches(&r));
        for query in ["\"unclosed", "| foo", "foo |", "size:no", "dm:yesterday", "regex:["] { assert!(Filter::parse(query,false).is_err(),"{query}"); }
    }
    #[test] fn filtered_navigation_preserves_source_indices() {
        let mut app=App::new(None);
        app.files.all_rows=vec![row("keep.jpg",1),row("other.png",1)];
        app.files.search.query="ext:jpg".into();
        app.apply_file_filter();
        assert_eq!(app.files.rows.len(),1);assert_eq!(app.media_navigation(),vec![7]);
        app.files.search.query="regex:[".into();app.apply_file_filter();
        assert_eq!(app.files.rows.len(),1);assert!(app.files.search.message.contains("Invalid regex"));
    }
    #[test] fn scoped_queries_and_navigation_cancel_global_search() {
        let q=global_query("cat | dog",1,Some(Path::new("C:/media")));
        assert!(q.contains("<cat | dog>"));assert!(q.contains("path:\"C:/media\\\""));assert!(q.starts_with("ext:"));
        let mut state=State::default();state.scope=1;state.query="pending".into();state.navigation_reset();
        assert_eq!(state.scope,0);assert!(state.query.is_empty());
    }
}

#[cfg(test)]
mod results_tests {
    use super::*;
    #[test] fn mixed_global_results_keep_paths_and_thumbnail_mapping() {
        let ctx=egui::Context::default();let mut app=App::new(None);
        let hits=[("C:/different/folder",true),("D:/photos/a.jpg",false),("E:/docs/a.pdf",false),("D:/other/a.xyz",false),("E:/comic.cbz",false)].into_iter().map(|(path,folder)|everything::Hit{path:path.into(),folder,bytes:Some(42),created:String::new(),modified:String::new()}).collect();
        app.install_search_results(hits,&ctx);
        assert!(app.files.search.results);assert_eq!(app.images.len(),2);assert_eq!(app.gallery_nodes.len(),2);
        assert_eq!(app.files.rows[0].thumb,Some(2));assert_eq!(app.files.rows[1].media,Some(0));assert_eq!(app.files.rows[2].media,Some(1));assert_eq!(app.files.rows[4].thumb,Some(3));
        assert_eq!(app.files.rows[1].path.as_deref(),Some(Path::new("D:/photos/a.jpg")));assert!(app.files.rows[3].media.is_none());
    }
}
