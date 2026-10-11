//! Bounded, read-only Markdown cards. Full documents use the existing CommonMark reader.
use super::*;
use egui::text::{LayoutJob, TextFormat};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

const MAX_MARKDOWN: u64 = 4 * 1024 * 1024;
#[derive(Clone, Default)]
struct Style {
    heading: u8,
    strong: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: bool,
    quote: bool,
}
pub struct Markdown {
    pub text: String,
    spans: Vec<(String, Style)>,
    cache: egui_commonmark::CommonMarkCache,
}
impl Markdown {
    pub fn load(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_MARKDOWN + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() as u64 <= MAX_MARKDOWN, "Markdown exceeds 4 MiB");
        let text = String::from_utf8(bytes).context("Markdown must be UTF-8")?;
        Ok(Self::parse(text.trim_start_matches('\u{feff}').to_owned()))
    }
    fn parse(text: String) -> Self {
        let mut styles = vec![Style::default()];
        let mut spans = Vec::new();
        let mut lists = Vec::new();
        // Card parsing is bounded independently of the full document reader.
        let end = text.floor_char_boundary(text.len().min(32_768));
        let options =
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
        for event in Parser::new_ext(&text[..end], options) {
            let mut style = styles.last().cloned().unwrap_or_default();
            match event {
                Event::Start(tag) => {
                    match tag {
                        Tag::Heading { level, .. } => style.heading = level as u8,
                        Tag::Strong | Tag::TableHead => style.strong = true,
                        Tag::Emphasis => style.italic = true,
                        Tag::CodeBlock(_) => style.code = true,
                        Tag::Strikethrough => style.strike = true,
                        Tag::Link { .. } => style.link = true,
                        Tag::BlockQuote(_) => {
                            style.quote = true;
                            spans.push(("> ".into(), style.clone()));
                        }
                        Tag::List(start) => lists.push(start),
                        Tag::Item => {
                            let indent = "  ".repeat(lists.len().saturating_sub(1).min(8));
                            let marker = if let Some(Some(n)) = lists.last_mut() {
                                let marker = format!("{n}. ");
                                *n += 1;
                                marker
                            } else {
                                "• ".into()
                            };
                            spans.push((format!("{indent}{marker}"), style.clone()));
                        }
                        Tag::Image { .. } => spans.push(("[Image: ".into(), style.clone())),
                        _ => {}
                    }
                    styles.push(style);
                }
                Event::End(tag) => {
                    let suffix = match tag {
                        TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_) => "\n\n",
                        TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead => "\n",
                        TagEnd::TableCell => "   ",
                        TagEnd::Image => "]",
                        _ => "",
                    };
                    if !suffix.is_empty() {
                        spans.push((suffix.into(), Style::default()));
                    }
                    if matches!(tag, TagEnd::List(_)) {
                        lists.pop();
                    }
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                Event::Text(t) => spans.push((t.into_string(), style)),
                Event::Code(t) => {
                    style.code = true;
                    spans.push((t.into_string(), style));
                }
                Event::SoftBreak => spans.push((" ".into(), style)),
                Event::HardBreak => spans.push(("\n".into(), style)),
                Event::Rule => spans.push(("────────────\n".into(), style)),
                Event::TaskListMarker(checked) => {
                    spans.push(((if checked { "[x] " } else { "[ ] " }).into(), style))
                }
                // Raw HTML is not executed and cannot start remote requests from the card.
                _ => {}
            }
        }
        Self {
            text,
            spans,
            cache: Default::default(),
        }
    }
    pub fn paint(
        &self,
        painter: &egui::Painter,
        object: &Object,
        camera: Camera,
        area: Rect,
        path: &Path,
    ) {
        let size = Vec2::from(object.size) * camera.zoom;
        let corners = object.corners().map(|p| camera.screen(p, area));
        painter.add(egui::Shape::convex_polygon(
            corners.to_vec(),
            Color32::from_rgb(30, 37, 48),
            egui::Stroke::new(1.0_f32, Color32::from_rgb(68, 84, 106)),
        ));
        if size.x < 35.0 || size.y < 35.0 {
            return;
        }
        let scale = size.x / 420.0;
        let pad = 16.0 * scale;
        let font = (16.0 * scale).clamp(2.0, 96.0);
        let width = (size.x - pad * 2.0).max(1.0);
        let available_height = (size.y - 82.0 * scale).max(1.0);
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        job.wrap.max_rows = (available_height / (font * 1.9)).floor().max(1.0) as usize;
        for (text, style) in &self.spans {
            let heading_scale = match style.heading {
                1 => 1.65,
                2 => 1.4,
                3 => 1.2,
                _ => 1.0,
            };
            let color = if style.link {
                Color32::from_rgb(110, 192, 240)
            } else if style.heading > 0 || style.strong {
                Color32::WHITE
            } else if style.quote {
                Color32::from_rgb(168, 183, 199)
            } else {
                Color32::from_rgb(210, 220, 230)
            };
            job.append(
                text,
                0.0,
                TextFormat {
                    font_id: if style.code {
                        FontId::monospace(font * 0.92)
                    } else {
                        FontId::proportional(font * heading_scale)
                    },
                    color,
                    italics: style.italic,
                    background: if style.code {
                        Color32::from_rgb(19, 25, 34)
                    } else {
                        Color32::TRANSPARENT
                    },
                    underline: if style.link {
                        egui::Stroke::new(scale.max(0.4), color)
                    } else {
                        egui::Stroke::NONE
                    },
                    strikethrough: if style.strike {
                        egui::Stroke::new(scale.max(0.4), color)
                    } else {
                        egui::Stroke::NONE
                    },
                    ..Default::default()
                },
            );
        }
        let mut title = LayoutJob::simple(
            file_name(path),
            FontId::proportional(font * 0.85),
            Color32::from_rgb(132, 192, 233),
            width,
        );
        title.wrap.max_rows = 1;
        let title = painter.layout_job(title);
        let content = painter.layout_job(job);
        let footer = painter.layout_no_wrap(
            "MD · Double-click to read".into(),
            FontId::proportional(font * 0.75),
            ui_theme::MUTED,
        );
        for (galley, offset) in [
            (title, Vec2::new(pad, pad)),
            (content, Vec2::new(pad, 44.0 * scale)),
            (footer, Vec2::new(pad, size.y - 26.0 * scale)),
        ] {
            painter.add(
                egui::epaint::TextShape::new(
                    corners[0] + rotate(offset, object.angle),
                    galley,
                    Color32::WHITE,
                )
                .with_angle(object.angle),
            );
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.cache, &self.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn card_parses_markdown_instead_of_displaying_markup_and_limits_input() {
        let doc = Markdown::parse("# Heading\n\n**Strong** and *italic*, `code`.\n\n- item\n- [x] task\n\n[link](https://example.com)\n\n<script>ignored</script>".into());
        assert!(doc
            .spans
            .iter()
            .any(|(t, s)| t == "Heading" && s.heading == 1));
        assert!(doc.spans.iter().any(|(t, s)| t == "Strong" && s.strong));
        assert!(doc.spans.iter().any(|(t, s)| t == "italic" && s.italic));
        assert!(doc.spans.iter().any(|(t, s)| t == "code" && s.code));
        assert!(doc.spans.iter().any(|(t, s)| t == "link" && s.link));
        assert!(!doc.spans.iter().any(|(t, _)| t.contains("<script>")));
        let root = super::super::tests::fixture("markdown-bounds");
        let file = root.join("notes.md");
        std::fs::write(&file, "\u{feff}# Привет\nText").unwrap();
        assert!(Markdown::load(&file).unwrap().text.starts_with("# Привет"));
        std::fs::write(&file, vec![b'a'; MAX_MARKDOWN as usize + 1]).unwrap();
        assert!(Markdown::load(&file).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
