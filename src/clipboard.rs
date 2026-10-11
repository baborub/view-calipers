//! Explorer-compatible CF_HDROP clipboard, including cut/copy drop effect.
use anyhow::{bail, Result};
use std::path::PathBuf;
#[derive(Clone, Debug)]
pub struct Files {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}
#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::ffi::{OsStrExt, OsStringExt},
    };
    type Handle = *mut c_void;
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(window: Handle) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, data: Handle) -> Handle;
        fn GetClipboardData(format: u32) -> Handle;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn GetActiveWindow() -> Handle;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, size: usize) -> Handle;
        fn GlobalLock(data: Handle) -> Handle;
        fn GlobalSize(data: Handle) -> usize;
        fn GlobalUnlock(data: Handle) -> i32;
        fn GlobalFree(data: Handle) -> Handle;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(drop: Handle, index: u32, text: *mut u16, len: u32) -> u32;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    unsafe fn open() -> Result<Guard> {
        if OpenClipboard(GetActiveWindow()) == 0 {
            bail!("Clipboard is busy; retry");
        }
        Ok(Guard)
    }
    unsafe fn effect() -> u32 {
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        RegisterClipboardFormatW(name.as_ptr())
    }
    unsafe fn set(format: u32, bytes: &[u8]) -> Result<()> {
        let data = GlobalAlloc(2, bytes.len());
        if data.is_null() {
            bail!("Cannot allocate clipboard data");
        }
        let pointer = GlobalLock(data);
        if pointer.is_null() {
            GlobalFree(data);
            bail!("Cannot lock clipboard data");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
        GlobalUnlock(data);
        if SetClipboardData(format, data).is_null() {
            GlobalFree(data);
            bail!("Cannot set clipboard data");
        }
        Ok(())
    }
    pub fn write(files: &Files) -> Result<()> {
        unsafe {
            if GetActiveWindow().is_null() {
                bail!("Activate view-calipers before copying files");
            }
            let _guard = open()?;
            if EmptyClipboard() == 0 {
                bail!("Cannot empty clipboard");
            }
            let mut bytes = vec![0u8; 20];
            bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
            bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
            for path in &files.paths {
                for unit in path.as_os_str().encode_wide().chain(Some(0)) {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&0u16.to_le_bytes());
            if files.paths.is_empty() {
                bytes.extend_from_slice(&0u16.to_le_bytes());
            }
            set(15, &bytes)?;
            set(
                effect(),
                &(if files.cut { 2u32 } else { 1u32 }).to_le_bytes(),
            )?;
            Ok(())
        }
    }
    pub fn read() -> Result<Files> {
        unsafe {
            let _guard = open()?;
            let drop = GetClipboardData(15);
            if drop.is_null() {
                bail!("Clipboard does not contain files");
            }
            let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
            let mut paths = Vec::new();
            for index in 0..count {
                let len = DragQueryFileW(drop, index, std::ptr::null_mut(), 0);
                let mut text = vec![0u16; len as usize + 1];
                DragQueryFileW(drop, index, text.as_mut_ptr(), len + 1);
                paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                    &text[..len as usize],
                )));
            }
            let data = GetClipboardData(effect());
            let mut cut = false;
            if !data.is_null() && GlobalSize(data) >= 4 {
                let p = GlobalLock(data) as *const u32;
                if !p.is_null() {
                    cut = *p & 2 != 0;
                    GlobalUnlock(data);
                }
            }
            Ok(Files { paths, cut })
        }
    }
}
#[cfg(windows)]
pub use native::{read, write};
