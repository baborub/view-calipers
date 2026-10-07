use std::path::Path;
use anyhow::{anyhow, Result};

pub struct PdfScrubber {
    pending: std::sync::Arc<(std::sync::Mutex<Option<(u32, u32)>>, std::sync::Condvar)>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    frames: std::sync::mpsc::Receiver<(u32, u32, Result<eframe::egui::ColorImage>)>,
}
impl PdfScrubber {
    pub fn new(path: std::path::PathBuf, ctx: eframe::egui::Context) -> Self {
        use std::sync::{Arc, Mutex, Condvar, atomic::{AtomicBool, Ordering}, mpsc};
        let pending = Arc::new((Mutex::new(None::<(u32, u32)>), Condvar::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (tx, frames) = mpsc::channel();
        let queue = pending.clone(); let stop = stopped.clone();
        std::thread::spawn(move || {
            let mut cache = std::collections::VecDeque::<((u32, u32), eframe::egui::ColorImage)>::new();
            loop {
                let mut request = queue.0.lock().unwrap();
                while request.is_none() && !stop.load(Ordering::Relaxed) { request = queue.1.wait(request).unwrap(); }
                if stop.load(Ordering::Relaxed) { break; }
                let (page, width) = request.take().unwrap(); drop(request);
                let result = if let Some((_, image)) = cache.iter().find(|(key, _)| *key == (page, width)) {
                    Ok(image.clone())
                } else {
                    pdf_page(&path, page, width).and_then(|(_, bytes)| {
                        let rgba = image::load_from_memory(&bytes)?.to_rgba8();
                        let image = eframe::egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                        cache.push_back(((page, width), image.clone()));
                        if cache.len() > 6 { cache.pop_front(); }
                        Ok(image)
                    })
                };
                if stop.load(Ordering::Relaxed) || tx.send((page, width, result)).is_err() { break; }
                ctx.request_repaint();
            }
        });
        Self { pending, stopped, frames }
    }
    pub fn seek(&self, page: u32, width: u32) { *self.pending.0.lock().unwrap() = Some((page, width)); self.pending.1.notify_one(); }
    pub fn poll(&self) -> Option<(u32, u32, Result<eframe::egui::ColorImage>)> { self.frames.try_iter().last() }
}
impl Drop for PdfScrubber {
    fn drop(&mut self) {
        let _guard = self.pending.0.lock().unwrap();
        self.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
        self.pending.1.notify_one();
    }
}

pub fn pdf_page(path: &Path, index: u32, width: u32) -> Result<(u32, Vec<u8>)> {
    let path = std::fs::canonicalize(path)?;
    std::thread::spawn(move || -> Result<(u32, Vec<u8>)> {
        use windows::{core::HSTRING, Data::Pdf::{PdfDocument, PdfPageRenderOptions}, Storage::{StorageFile, Streams::{InMemoryRandomAccessStream, DataReader}}, Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED}};
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)?; }
        struct Runtime;
        impl Drop for Runtime { fn drop(&mut self) { unsafe { RoUninitialize(); } } }
        let _runtime = Runtime;
        let path = path.to_string_lossy();
        let path = if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") { format!("\\\\{unc}") }
            else { path.trim_start_matches("\\\\?\\").to_string() };
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.get()?;
        let document = PdfDocument::LoadFromFileAsync(&file)?.get()?;
        let count = document.PageCount()?;
        if index >= count { return Err(anyhow!("PDF page does not exist")); }
        let page = document.GetPage(index)?;
        let stream = InMemoryRandomAccessStream::new()?;
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(width)?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?.get()?;
        let size = u32::try_from(stream.Size()?)?;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.get()?;
        let mut bytes = vec![0; size as usize];
        reader.ReadBytes(&mut bytes)?;
        reader.Close()?;
        page.Close()?;
        Ok((count, bytes))
    }).join().map_err(|_| anyhow!("PDF rendering thread failed"))?
}

pub fn markdown_thumbnail(text: &str) -> image::RgbaImage {
    let mut image = image::RgbaImage::from_pixel(180, 240, image::Rgba([238, 241, 246, 255]));
    for (line, content) in text.lines().filter(|line| !line.trim().is_empty()).take(16).enumerate() {
        let y = 18 + line as u32 * 13;
        let heading = content.starts_with('#');
        let width = (content.chars().count() as u32 * 5).clamp(25, 148);
        let color = if heading { image::Rgba([45, 105, 160, 255]) } else { image::Rgba([130, 140, 155, 255]) };
        for x in 16..16 + width { for row in y..y + if heading { 5 } else { 3 } { image.put_pixel(x, row, color); } }
    }
    image
}
