//! External image import. Only explicit drops/pastes download or create owned images.
use super::*;
pub mod native;
pub use native::NativeDrop;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Input {
    Files(Vec<PathBuf>),
    Bytes { name: String, bytes: Vec<u8> },
    Url(String),
}
pub fn image_url(text: &str) -> Option<String> {
    let text = text.trim().trim_matches('\0');
    if text.len() > MAX_BYTES * 2 {
        return None;
    }
    if text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("data:image/")
    {
        return Some(text.lines().next()?.trim().to_owned());
    }
    static SRC: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = SRC.get_or_init(|| {
        regex::Regex::new(r#"(?is)<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["']"#).unwrap()
    });
    let url = re
        .captures(text)?
        .get(1)?
        .as_str()
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"");
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:image/") {
        Some(url)
    } else {
        None
    }
}
fn data_url(url: &str) -> Result<Vec<u8>> {
    let (header, data) = url.split_once(',').context("Invalid image data URL")?;
    anyhow::ensure!(
        header.starts_with("data:image/") && header.ends_with(";base64"),
        "Unsupported image data URL"
    );
    let mut bytes = Vec::new();
    let mut value = 0u32;
    let mut bits = 0;
    for c in data.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let d = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => anyhow::bail!("Invalid base64 image"),
        };
        value = (value << 6) | d as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((value >> bits) as u8);
        }
        anyhow::ensure!(
            bytes.len() <= MAX_BYTES,
            "Image exceeds 64 MiB import limit"
        );
    }
    Ok(bytes)
}
pub fn import(input: Input, directory: &Path) -> Result<Vec<PathBuf>> {
    match input {
        Input::Files(paths) => paths
            .into_iter()
            .filter(|p| is_image_ext(&ext_of_path(p)))
            .map(|p| {
                anyhow::ensure!(
                    std::fs::metadata(&p)?.len() <= MAX_BYTES as u64,
                    "Image exceeds 64 MiB import limit"
                );
                save_image(directory, &file_name(&p), &std::fs::read(p)?)
            })
            .collect(),
        Input::Bytes { name, bytes } => Ok(vec![save_image(directory, &name, &bytes)?]),
        Input::Url(url) => {
            let url =
                image_url(&url).context("Drop a direct image link or copy the image itself")?;
            let bytes = if url.starts_with("data:") {
                data_url(&url)?
            } else {
                native::download(&url)?
            };
            let name = url
                .split('?')
                .next()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("image");
            Ok(vec![save_image(directory, name, &bytes)?])
        }
    }
}
pub fn save_image(directory: &Path, suggested: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES,
        "Image exceeds 64 MiB import limit or is empty"
    );
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let format = reader
        .format()
        .context("The dropped content is not an image (it may be a webpage)")?;
    let (w, h) = reader.into_dimensions().context("Invalid image data")?;
    anyhow::ensure!(
        w > 0 && h > 0 && u64::from(w) * u64::from(h) <= 256_000_000,
        "Image dimensions exceed the import limit"
    );
    let ext = if format == image::ImageFormat::Jpeg {
        "jpg"
    } else {
        format.extensions_str().first().copied().unwrap_or("png")
    };
    anyhow::ensure!(is_image_ext(ext), "Unsupported image format");
    let stem = Path::new(suggested)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let stem: String = stem
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"/\\|?*%".contains(*c))
        .take(80)
        .collect();
    let stem = if fileops::valid_name(&stem).is_ok() {
        stem
    } else {
        "image".into()
    };
    std::fs::create_dir_all(directory)?;
    for i in 0..100_000 {
        let path = directory.join(if i == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{i}.{ext}")
        });
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e.into());
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Could not choose a free image filename")
}
pub fn clipboard() -> Result<Input> {
    if let Ok(files) = crate::clipboard::read() {
        if !files.paths.is_empty() {
            return Ok(Input::Files(files.paths));
        }
    }
    let mut clipboard = arboard::Clipboard::new()?;
    if let Ok(image) = clipboard.get_image() {
        anyhow::ensure!(
            image.width.saturating_mul(image.height) <= 256_000_000,
            "Clipboard image is too large"
        );
        let image = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        )
        .context("Invalid clipboard image")?;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut bytes, image::ImageFormat::Png)?;
        return Ok(Input::Bytes {
            name: "pasted-image.png".into(),
            bytes: bytes.into_inner(),
        });
    }
    if let Ok(input) = native::clipboard() {
        return Ok(input);
    }
    let text = clipboard.get_text()?;
    Ok(Input::Url(
        image_url(&text).context("Clipboard contains no image or image URL")?,
    ))
}
impl App {
    pub(crate) fn receive_external_images(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.external_drop.is_none() {
            match NativeDrop::install(frame, ctx) {
                Ok(drop) => self.external_drop = Some(drop),
                Err(error) => {
                    self.file_message = format!("External drag-and-drop: {error}");
                }
            }
        }
        let events = self
            .external_drop
            .as_mut()
            .map(|d| d.poll())
            .unwrap_or_default();
        for (point, result) in events {
            let input = match result {
                Ok(input) => input,
                Err(e) => {
                    self.file_message = e;
                    continue;
                }
            };
            let editor = self
                .canvas
                .editor
                .as_mut()
                .filter(|e| e.area.contains(point))
                .or_else(|| self.board.as_mut().filter(|e| e.area.contains(point)))
                .or_else(|| {
                    self.inactive
                        .as_mut()
                        .and_then(|a| a.board.as_mut())
                        .filter(|e| e.area.contains(point))
                });
            if let Some(editor) = editor {
                match input {
                    Input::Files(paths) => editor.add_files(&paths, Some(point)),
                    other => editor.import_images(other, Some(point), ctx),
                }
            } else if let Input::Files(paths) = input {
                if let Some(path) = paths.first() {
                    self.open_path(path, ctx);
                }
            } else {
                self.file_message =
                    "Select a collection and drop the image onto its canvas.".into();
            }
        }
        let areas: Vec<_> = [
            self.canvas.editor.as_ref(),
            self.board.as_ref(),
            self.inactive.as_ref().and_then(|a| a.board.as_ref()),
        ]
        .into_iter()
        .flatten()
        .map(|e| e.area)
        .collect();
        if let Some(drop) = &mut self.external_drop {
            drop.set_areas(areas);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn png() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            24,
            36,
            image::Rgb([70, 130, 190]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        bytes.into_inner()
    }
    #[test]
    fn browser_image_urls_and_imports_are_local_collision_safe_and_validated() {
        assert_eq!(
            image_url(r#"<img alt='test' src="https://example.com/image?a=1&amp;b=2">"#).unwrap(),
            "https://example.com/image?a=1&b=2"
        );
        assert!(image_url("javascript:alert(1)").is_none());
        assert_eq!(
            data_url("data:image/png;base64,AQIDBA==").unwrap(),
            vec![1, 2, 3, 4]
        );
        let root = std::env::temp_dir().join(format!("vc-browser-import-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = save_image(&root, "../image.jpg", &png()).unwrap();
        let second = save_image(&root, "../image.jpg", &png()).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.extension().unwrap(), "png");
        assert_eq!(first.parent().unwrap(), root);
        assert!(save_image(&root, "page.png", b"<html>sign in</html>").is_err());
        assert!(!root.join("page.png").exists());
        let source = root.join("source.png");
        std::fs::write(&source, png()).unwrap();
        let owned = import(Input::Files(vec![source.clone()]), &root.join("collection")).unwrap();
        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(&owned[0]).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn url_download_runs_through_http_and_rejects_error_pages() {
        use std::io::{Read, Write};
        for (code, body, valid) in [
            (200, png(), true),
            (200, b"<html>not an image</html>".to_vec(), false),
            (403, png(), false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let _ = socket.read(&mut request);
                write!(
                    socket,
                    "HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            });
            let root = std::env::temp_dir().join(format!(
                "vc-http-import-{}-{code}-{valid}",
                std::process::id()
            ));
            let result = import(Input::Url(format!("http://{address}/image.png")), &root);
            assert_eq!(result.is_ok(), valid, "{result:?}");
            server.join().unwrap();
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}
