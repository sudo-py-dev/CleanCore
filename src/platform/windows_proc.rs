//! Windows processes: ToolHelp snapshot, visible windows, TerminateProcess, WindowFromPoint,
//! and the publisher from a file's version resource.

use super::Proc;
use anyhow::{bail, Result};
use std::collections::HashMap;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetAncestor, GetCursorPos, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WindowFromPoint, GA_ROOT};

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: handle opened and closed here; buffer length passed in.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> i32 {
    let map = &mut *(lparam as *mut HashMap<u32, String>);
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut buf = [0u16; 512];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    map.entry(pid).or_insert_with(|| String::from_utf16_lossy(&buf[..n as usize]));
    1
}

fn windows_by_pid() -> HashMap<u32, String> {
    let mut map: HashMap<u32, String> = HashMap::new();
    // SAFETY: the callback only touches `map`, which outlives the call.
    unsafe {
        EnumWindows(Some(collect), &mut map as *mut _ as LPARAM);
    }
    map
}

pub fn list_processes() -> Vec<Proc> {
    let titles = windows_by_pid();
    let mut out = Vec::new();
    // SAFETY: snapshot handle closed below; entry size set as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            let pid = e.th32ProcessID;
            let title = titles.get(&pid).cloned();
            out.push(Proc { pid, exe: image_path(pid), cmdline: String::new(), gui: title.is_some(), app: title.is_some(), title, name });
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

pub fn kill_process(pid: u32) -> Result<()> {
    // SAFETY: handle opened and closed here.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            bail!("cannot open process {pid} (run as administrator?)");
        }
        let ok = TerminateProcess(h, 1);
        CloseHandle(h);
        if ok == 0 {
            bail!("could not stop process {pid}");
        }
    }
    Ok(())
}

pub fn window_at_pointer() -> Result<Option<u32>> {
    // SAFETY: plain user32 queries.
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt) == 0 {
            return Ok(None);
        }
        let hwnd = WindowFromPoint(pt);
        if hwnd.is_null() {
            return Ok(None);
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let mut pid = 0u32;
        GetWindowThreadProcessId(if root.is_null() { hwnd } else { root }, &mut pid);
        Ok((pid != 0).then_some(pid))
    }
}

/// CompanyName from the file's version resource.
pub fn file_publishers(paths: &[String]) -> std::collections::HashMap<String, String> {
    paths.iter().filter_map(|p| Some((p.clone(), file_publisher(p)?))).collect()
}

pub fn file_publisher(path: &str) -> Option<String> {
    version_string(path, "CompanyName")
}

/// The program's own name (FileDescription): "Microsoft OneDrive" for OneDrive.exe.
pub fn file_description(path: &str) -> Option<String> {
    version_string(path, "FileDescription")
}

/// Milliseconds since Windows started.
pub fn uptime_ms() -> Option<u64> {
    // SAFETY: no arguments.
    Some(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() })
}

/// Age, CPU time and disk/IO bytes of a process.
pub fn process_usage(pid: u32) -> Option<crate::autorun::ProcUsage> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{GetProcessIoCounters, GetProcessTimes, IO_COUNTERS};
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
    // SAFETY: handle opened and closed here; the out structs are plain data.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user, mut now) = (z(), z(), z(), z(), z());
        let mut io: IO_COUNTERS = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        let io_ok = GetProcessIoCounters(h, &mut io) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        GetSystemTimeAsFileTime(&mut now);
        Some(crate::autorun::ProcUsage { age_ms: ft(now).saturating_sub(ft(created)), cpu_ms: ft(kernel) + ft(user), io_bytes: if io_ok { io.ReadTransferCount } else { 0 } })
    }
}

fn version_string(path: &str, field: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    // SAFETY: buffers sized by the API; pointers returned by VerQueryValueW point into `data`.
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr() as *mut _) == 0 {
            return None;
        }
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let q: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain([0]).collect();
        let (lang, cp) = if VerQueryValueW(data.as_ptr() as *const _, q.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let t = ptr as *const u16;
            (*t, *t.add(1))
        } else {
            (0x0409, 0x04b0)
        };
        let key: Vec<u16> = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{field}").encode_utf16().chain([0]).collect();
        if VerQueryValueW(data.as_ptr() as *const _, key.as_ptr(), &mut ptr, &mut len) == 0 || len == 0 {
            return None;
        }
        let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
        let v = String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
        (!v.is_empty()).then_some(v)
    }
}

/// RGBA pixels of icon `index` in an `.exe` / `.dll` / `.ico` (the large, 32-px system icon).
pub fn icon_rgba(file: &std::path::Path, index: i32) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};
    let w: Vec<u16> = file.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let mut large: HICON = std::ptr::null_mut();
    // SAFETY: `w` is NUL-terminated; we ask for one large icon and destroy what we get.
    let n = unsafe { ExtractIconExW(w.as_ptr(), index, &mut large, std::ptr::null_mut(), 1) };
    if n == 0 || large.is_null() {
        return None;
    }
    let out = hicon_rgba(large);
    // SAFETY: icon returned by ExtractIconExW above.
    unsafe { DestroyIcon(large) };
    out
}

fn hicon_rgba(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};
    // SAFETY: GDI calls on bitmaps returned by GetIconInfo, which we delete; buffers are sized
    // from the bitmap's own dimensions.
    unsafe {
        let mut info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 {
            return None;
        }
        let cleanup = |info: &ICONINFO| {
            if !info.hbmColor.is_null() {
                DeleteObject(info.hbmColor);
            }
            if !info.hbmMask.is_null() {
                DeleteObject(info.hbmMask);
            }
        };
        if info.hbmColor.is_null() {
            cleanup(&info);
            return None; // monochrome icon
        }
        let mut bm: BITMAP = std::mem::zeroed();
        GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut core::ffi::c_void);
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
            cleanup(&info);
            return None;
        }
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let read = |bmp| {
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h; // top-down rows
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut px = vec![0u8; (w * h * 4) as usize];
            let ok = GetDIBits(dc, bmp, 0, h as u32, px.as_mut_ptr() as *mut core::ffi::c_void, &mut bi, DIB_RGB_COLORS);
            (ok != 0).then_some(px)
        };
        let color = read(info.hbmColor);
        // Icons without an alpha channel use the AND mask for transparency.
        let mask = if color.as_ref().is_some_and(|c| c.chunks(4).all(|p| p[3] == 0)) && !info.hbmMask.is_null() { read(info.hbmMask) } else { None };
        DeleteDC(dc);
        cleanup(&info);
        let mut px = color?;
        for (i, p) in px.chunks_mut(4).enumerate() {
            p.swap(0, 2); // BGRA → RGBA
            if let Some(m) = &mask {
                p[3] = if m[i * 4] == 0 { 255 } else { 0 };
            }
        }
        Some((w as u32, h as u32, px))
    }
}
