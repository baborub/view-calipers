use super::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{implement, w, PCWSTR};
use windows::Win32::{
    Foundation::{HWND, POINT, POINTL},
    Graphics::Gdi::ScreenToClient,
    Networking::WinHttp::*,
    System::{
        Com::*, DataExchange::RegisterClipboardFormatW, Memory::*, Ole::*,
        SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::Shell::{DragQueryFileW, FILEDESCRIPTORW, HDROP},
};

type Event = (Pos2, std::result::Result<Input, String>);
pub struct NativeDrop {
    hwnd: HWND,
    _target: IDropTarget,
    events: Arc<Mutex<VecDeque<Event>>>,
    areas: Arc<Mutex<Vec<Rect>>>,
}
impl NativeDrop {
    pub fn install(frame: &eframe::Frame, ctx: &egui::Context) -> Result<Self> {
        let RawWindowHandle::Win32(handle) = frame.window_handle()?.as_raw() else {
            anyhow::bail!("Windows window handle unavailable");
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        unsafe {
            OleInitialize(None)?;
            let events = Arc::new(Mutex::new(VecDeque::new()));
            let areas = Arc::new(Mutex::new(Vec::new()));
            let target: IDropTarget = DropTarget {
                hwnd: hwnd.0 as usize,
                ctx: ctx.clone(),
                events: events.clone(),
                areas: areas.clone(),
                kind: std::cell::Cell::new(0),
            }
            .into();
            if let Err(e) = RegisterDragDrop(hwnd, &target) {
                OleUninitialize();
                return Err(e.into());
            }
            Ok(Self {
                hwnd,
                _target: target,
                events,
                areas,
            })
        }
    }
    pub fn poll(&mut self) -> Vec<Event> {
        self.events.lock().unwrap().drain(..).collect()
    }
    pub fn set_areas(&mut self, areas: Vec<Rect>) {
        *self.areas.lock().unwrap() = areas;
    }
}
impl Drop for NativeDrop {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            OleUninitialize();
        }
    }
}
#[implement(IDropTarget)]
struct DropTarget {
    hwnd: usize,
    ctx: egui::Context,
    events: Arc<Mutex<VecDeque<Event>>>,
    areas: Arc<Mutex<Vec<Rect>>>,
    kind: std::cell::Cell<u8>,
}
impl DropTarget {
    fn point(&self, pt: &POINTL) -> Pos2 {
        let mut pt = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(HWND(self.hwnd as *mut _), &mut pt);
        }
        Pos2::new(pt.x as f32, pt.y as f32) / self.ctx.pixels_per_point()
    }
    fn allowed(&self, pt: &POINTL) -> bool {
        self.kind.get() == 1
            || (self.kind.get() == 2
                && self
                    .areas
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|a| a.contains(self.point(pt))))
    }
}
impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Option<&IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let kind = data
            .map(|d| unsafe {
                if d.QueryGetData(&format(15, -1, TYMED_HGLOBAL)).is_ok() {
                    1
                } else if [
                    "FileGroupDescriptorW",
                    "PNG",
                    "HTML Format",
                    "UniformResourceLocatorW",
                    "UniformResourceLocator",
                ]
                .iter()
                .any(|name| {
                    d.QueryGetData(&format(registered(name), -1, TYMED_HGLOBAL))
                        .is_ok()
                }) || d.QueryGetData(&format(13, -1, TYMED_HGLOBAL)).is_ok()
                {
                    2
                } else {
                    0
                }
            })
            .unwrap_or(0);
        self.kind.set(kind);
        self.DragOver(MODIFIERKEYS_FLAGS(0), pt, effect)
    }
    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            if let Some(effect) = effect.as_mut() {
                *effect = if self.allowed(pt) {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
        }
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        self.kind.set(0);
        Ok(())
    }
    fn Drop(
        &self,
        data: Option<&IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let allowed = self.allowed(pt);
        unsafe {
            if let Some(effect) = effect.as_mut() {
                *effect = if allowed {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
        }
        if allowed {
            let result = data
                .context("Missing drag data")
                .and_then(read_object)
                .map_err(|e| e.to_string());
            self.events
                .lock()
                .unwrap()
                .push_back((self.point(pt), result));
            self.ctx.request_repaint();
        }
        self.kind.set(0);
        Ok(())
    }
}
fn registered(name: &str) -> u16 {
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) as u16 }
}
fn format(id: u16, index: i32, ty: TYMED) -> FORMATETC {
    FORMATETC {
        cfFormat: id,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index,
        tymed: ty.0 as u32,
        ..Default::default()
    }
}
struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe {
            ReleaseStgMedium(&mut self.0);
        }
    }
}
fn medium(data: &IDataObject, id: u16, index: i32) -> Result<Medium> {
    unsafe {
        Ok(Medium(data.GetData(&format(
            id,
            index,
            TYMED(TYMED_HGLOBAL.0 | TYMED_ISTREAM.0),
        ))?))
    }
}
fn bytes(data: &IDataObject, id: u16, index: i32) -> Result<Vec<u8>> {
    let medium = medium(data, id, index)?;
    unsafe {
        if medium.0.tymed == TYMED_HGLOBAL.0 as u32 {
            let handle = medium.0.u.hGlobal;
            let size = GlobalSize(handle);
            anyhow::ensure!(
                size > 0 && size <= MAX_BYTES,
                "Dropped content is empty or exceeds 64 MiB"
            );
            let pointer = GlobalLock(handle);
            anyhow::ensure!(!pointer.is_null(), "Cannot read dropped content");
            let bytes = std::slice::from_raw_parts(pointer as *const u8, size).to_vec();
            let _ = GlobalUnlock(handle);
            Ok(bytes)
        } else if medium.0.tymed == TYMED_ISTREAM.0 as u32 {
            let stream = medium.0.u.pstm.as_ref().context("Missing image stream")?;
            let mut result = Vec::new();
            let mut buffer = [0u8; 65536];
            loop {
                let mut count = 0;
                stream
                    .Read(
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        Some(&mut count),
                    )
                    .ok()?;
                if count == 0 {
                    break;
                }
                anyhow::ensure!(
                    result.len() + count as usize <= MAX_BYTES,
                    "Image exceeds 64 MiB"
                );
                result.extend_from_slice(&buffer[..count as usize]);
            }
            Ok(result)
        } else {
            anyhow::bail!("Unsupported dropped data")
        }
    }
}
fn wide(bytes: &[u8]) -> String {
    String::from_utf16_lossy(
        &bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|v| *v != 0)
            .collect::<Vec<_>>(),
    )
}
fn read_object(object: &IDataObject) -> Result<Input> {
    if let Ok(descriptor) = bytes(object, registered("FileGroupDescriptorW"), -1) {
        if descriptor.len() >= 4 + std::mem::size_of::<FILEDESCRIPTORW>()
            && u32::from_le_bytes(descriptor[..4].try_into().unwrap()) > 0
        {
            let descriptor = unsafe {
                std::ptr::read_unaligned(descriptor[4..].as_ptr().cast::<FILEDESCRIPTORW>())
            };
            let filename = descriptor.cFileName;
            let name = String::from_utf16_lossy(
                &filename[..filename.iter().position(|v| *v == 0).unwrap_or(260)],
            );
            if is_image_ext(&ext_of_name(&name)) {
                if let Ok(bytes) = bytes(object, registered("FileContents"), 0) {
                    return Ok(Input::Bytes { name, bytes });
                }
            }
        }
    }
    if let Ok(bytes) = bytes(object, registered("PNG"), -1) {
        return Ok(Input::Bytes {
            name: "browser-image.png".into(),
            bytes,
        });
    }
    if let Ok(medium) = medium(object, 15, -1) {
        unsafe {
            if medium.0.tymed == TYMED_HGLOBAL.0 as u32 {
                let drop = HDROP(medium.0.u.hGlobal.0);
                let count = DragQueryFileW(drop, u32::MAX, None);
                let mut paths = Vec::new();
                for i in 0..count.min(10000) {
                    let len = DragQueryFileW(drop, i, None);
                    let mut name = vec![0u16; len as usize + 1];
                    DragQueryFileW(drop, i, Some(&mut name));
                    paths.push(PathBuf::from(String::from_utf16_lossy(
                        &name[..len as usize],
                    )));
                }
                if paths.len() == 1
                    && paths[0].starts_with(std::env::temp_dir())
                    && is_image_ext(&ext_of_path(&paths[0]))
                {
                    anyhow::ensure!(
                        std::fs::metadata(&paths[0])?.len() <= MAX_BYTES as u64,
                        "Image exceeds 64 MiB"
                    );
                    return Ok(Input::Bytes {
                        name: file_name(&paths[0]),
                        bytes: std::fs::read(&paths[0])?,
                    });
                }
                if !paths.is_empty() {
                    return Ok(Input::Files(paths));
                }
            }
        }
    }
    for (id, utf16) in [
        (registered("HTML Format"), false),
        (registered("UniformResourceLocatorW"), true),
        (registered("UniformResourceLocator"), false),
        (13, true),
    ] {
        if let Ok(bytes) = bytes(object, id, -1) {
            let text = if utf16 {
                wide(&bytes)
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            if let Some(url) = image_url(&text) {
                return Ok(Input::Url(url));
            }
        }
    }
    anyhow::bail!("No image data in this drag. Try Copy image, then Ctrl+V on the canvas.")
}
pub fn clipboard() -> Result<Input> {
    unsafe { read_object(&OleGetClipboard()?) }
}

struct Internet(*mut std::ffi::c_void);
impl Internet {
    fn checked(handle: *mut std::ffi::c_void) -> Result<Self> {
        if handle.is_null() {
            Err(windows::core::Error::from_win32().into())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}
pub fn download(url: &str) -> Result<Vec<u8>> {
    unsafe {
        let encoded: Vec<u16> = url.encode_utf16().collect();
        let mut components = URL_COMPONENTS {
            dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
            dwHostNameLength: u32::MAX,
            dwUrlPathLength: u32::MAX,
            dwExtraInfoLength: u32::MAX,
            ..Default::default()
        };
        WinHttpCrackUrl(&encoded, 0, &mut components)?;
        anyhow::ensure!(
            components.nScheme == WINHTTP_INTERNET_SCHEME_HTTP
                || components.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS,
            "Only HTTP and HTTPS image links are supported"
        );
        let part = |p: windows::core::PWSTR, n: u32| {
            if n == 0 {
                String::new()
            } else {
                String::from_utf16_lossy(std::slice::from_raw_parts(p.0, n as usize))
            }
        };
        let host: Vec<u16> = part(components.lpszHostName, components.dwHostNameLength)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let resource = format!(
            "{}{}",
            part(components.lpszUrlPath, components.dwUrlPathLength),
            part(components.lpszExtraInfo, components.dwExtraInfoLength)
        );
        let resource: Vec<u16> = resource
            .split('#')
            .next()
            .unwrap_or("/")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let session = Internet::checked(WinHttpOpen(
            w!("view-calipers/0.4"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 10000, 10000, 15000, 15000)?;
        let connection = Internet::checked(WinHttpConnect(
            session.0,
            PCWSTR(host.as_ptr()),
            components.nPort,
            0,
        ))?;
        let flags = if components.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS {
            WINHTTP_FLAG_SECURE
        } else {
            WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        let request = Internet::checked(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(resource.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut status = 0u32;
        let mut size = 4;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            std::ptr::null_mut(),
        )?;
        anyhow::ensure!(
            (200..300).contains(&status),
            "Image download returned HTTP {status}. Use Copy image for pages requiring sign-in."
        );
        let mut result = Vec::new();
        let mut buffer = [0u8; 65536];
        let start = std::time::Instant::now();
        loop {
            let mut count = 0;
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut count,
            )?;
            if count == 0 {
                break;
            }
            anyhow::ensure!(
                result.len() + count as usize <= MAX_BYTES,
                "Image exceeds 64 MiB import limit"
            );
            anyhow::ensure!(start.elapsed().as_secs() < 60, "Image download timed out");
            result.extend_from_slice(&buffer[..count as usize]);
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{BOOL, DV_E_FORMATETC, E_NOTIMPL, S_OK};
    #[implement(IDataObject)]
    struct Data {
        formats: Vec<(u16, Vec<u8>)>,
    }
    impl IDataObject_Impl for Data_Impl {
        fn GetData(&self, f: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            unsafe {
                let f = &*f;
                let bytes = &self
                    .formats
                    .iter()
                    .find(|(id, _)| *id == f.cfFormat)
                    .ok_or_else(|| windows::core::Error::from(DV_E_FORMATETC))?
                    .1;
                let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
                let pointer = GlobalLock(handle);
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
                let _ = GlobalUnlock(handle);
                Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 { hGlobal: handle },
                    ..Default::default()
                })
            }
        }
        fn QueryGetData(&self, f: *const FORMATETC) -> windows::core::HRESULT {
            if unsafe { self.formats.iter().any(|(id, _)| *id == (*f).cfFormat) } {
                S_OK
            } else {
                DV_E_FORMATETC
            }
        }
        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetCanonicalFormatEtc(
            &self,
            _: *const FORMATETC,
            _: *mut FORMATETC,
        ) -> windows::core::HRESULT {
            E_NOTIMPL
        }
        fn SetData(
            &self,
            _: *const FORMATETC,
            _: *const STGMEDIUM,
            _: BOOL,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumFormatEtc(&self, _: u32) -> windows::core::Result<IEnumFORMATETC> {
            Err(E_NOTIMPL.into())
        }
        fn DAdvise(
            &self,
            _: *const FORMATETC,
            _: u32,
            _: Option<&IAdviseSink>,
        ) -> windows::core::Result<u32> {
            Err(E_NOTIMPL.into())
        }
        fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(E_NOTIMPL.into())
        }
    }
    #[test]
    fn windows_drop_target_accepts_virtual_browser_files_and_html_only_on_canvas() {
        let mut descriptor = FILEDESCRIPTORW::default();
        for (i, c) in "reference.png".encode_utf16().enumerate() {
            descriptor.cFileName[i] = c;
        }
        let mut packed = vec![0u8; 4 + std::mem::size_of::<FILEDESCRIPTORW>()];
        packed[..4].copy_from_slice(&1u32.to_le_bytes());
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&descriptor as *const FILEDESCRIPTORW).cast::<u8>(),
                packed[4..].as_mut_ptr(),
                std::mem::size_of::<FILEDESCRIPTORW>(),
            );
        }
        let object: IDataObject = Data {
            formats: vec![
                (registered("FileGroupDescriptorW"), packed),
                (registered("FileContents"), super::super::tests::png()),
            ],
        }
        .into();
        let Input::Bytes { name, bytes } = read_object(&object).unwrap() else {
            panic!()
        };
        assert_eq!(name, "reference.png");
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 24);
        let html: IDataObject = Data {
            formats: vec![(
                registered("HTML Format"),
                b"<img src=\"https://example.com/ref.png\">\0".to_vec(),
            )],
        }
        .into();
        assert!(
            matches!(read_object(&html).unwrap(),Input::Url(url) if url=="https://example.com/ref.png")
        );
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let ctx = egui::Context::default();
        let target: IDropTarget = DropTarget {
            hwnd: 0,
            ctx,
            events: events.clone(),
            areas: Arc::new(Mutex::new(vec![Rect::from_min_max(
                Pos2::new(10.0, 10.0),
                Pos2::new(200.0, 200.0),
            )])),
            kind: std::cell::Cell::new(0),
        }
        .into();
        let inside = POINTL { x: 50, y: 50 };
        let outside = POINTL { x: 300, y: 300 };
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            target
                .DragEnter(&object, MODIFIERKEYS_FLAGS(0), outside, &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
            target
                .DragOver(MODIFIERKEYS_FLAGS(0), inside, &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_COPY);
            target
                .Drop(&object, MODIFIERKEYS_FLAGS(0), inside, &mut effect)
                .unwrap();
        }
        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(matches!(
            events.lock().unwrap().pop_front().unwrap().1.unwrap(),
            Input::Bytes { .. }
        ));
    }
}
