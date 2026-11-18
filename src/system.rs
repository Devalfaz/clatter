//! Small Win32 helpers: process names, launch at login, single instance, theme.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_DWORD, RegCloseKey,
    RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcessId, INFINITE, OpenEventW,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

/// Lower-case executable file name of a process, e.g. `notepad.exe`.
pub fn exe_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&path).file_name().map(|n| n.to_string_lossy().to_lowercase())
    }
}

const SYSTEM_APPS: &[&str] = &[
    "applicationframehost.exe",
    "textinputhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "systemsettings.exe",
];

/// Executable names of apps that currently have a visible top-level window.
pub fn running_apps() -> Vec<String> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            let pids = &mut *(lparam.0 as *mut Vec<u32>);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let has_owner = GetWindow(hwnd, GW_OWNER).is_ok_and(|h| !h.is_invalid());
            if IsWindowVisible(hwnd).as_bool()
                && GetWindowTextLengthW(hwnd) > 0
                && ex & WS_EX_TOOLWINDOW.0 == 0
                && !has_owner
            {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                pids.push(pid);
            }
        }
        BOOL(1)
    }
    let mut pids: Vec<u32> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut pids as *mut _ as isize));
    }
    let own = unsafe { GetCurrentProcessId() };
    pids.sort();
    pids.dedup();
    let mut names: Vec<String> = pids
        .into_iter()
        .filter(|&p| p != own)
        .filter_map(exe_name)
        .filter(|n| !SYSTEM_APPS.contains(&n.as_str()))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Clatter");

/// Adds or removes Clatter from `HKCU\...\Run`.
pub fn set_launch_at_login(enabled: bool) -> windows::core::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).ok()?;
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|e| windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                e.to_string(),
            ))?;
            let cmd = wide(&format!("\"{}\" --background", exe.display()));
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
            RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)).ok()
        } else {
            let r = RegDeleteValueW(key, RUN_VALUE);
            if r.0 == 2 { Ok(()) } else { r.ok() } // ERROR_FILE_NOT_FOUND is fine
        };
        let _ = RegCloseKey(key);
        result
    }
}

/// True when the taskbar uses the light theme (so the tray glyph should be dark).
pub fn taskbar_is_light() -> bool {
    let mut value: u32 = 0;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && value != 0
}

const MUTEX_NAME: PCWSTR = w!("Local\\Clatter.Instance");
const SHOW_EVENT_NAME: PCWSTR = w!("Local\\Clatter.ShowSettings");

pub enum Instance {
    /// We are the only instance; keep the handle alive for the process lifetime.
    First(InstanceGuard),
    /// Another instance is running and has been asked to show its settings.
    AlreadyRunning,
}

pub struct InstanceGuard {
    _mutex: HANDLE,
    show_event: HANDLE,
}

unsafe impl Send for InstanceGuard {}

pub fn single_instance() -> Instance {
    unsafe {
        let mutex = CreateMutexW(None, true, MUTEX_NAME);
        let already = windows::Win32::Foundation::GetLastError().0 == 183; // ERROR_ALREADY_EXISTS
        match mutex {
            Ok(m) if !already => {
                let show_event = CreateEventW(None, false, false, SHOW_EVENT_NAME).unwrap_or_default();
                Instance::First(InstanceGuard { _mutex: m, show_event })
            }
            _ => {
                // Let the running copy bring its window to the front.
                let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                    windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
                );
                if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT_NAME) {
                    let _ = SetEvent(ev);
                    let _ = CloseHandle(ev);
                }
                Instance::AlreadyRunning
            }
        }
    }
}

impl InstanceGuard {
    /// Calls `on_show` (from a background thread) whenever another launch asks us to show settings.
    pub fn listen(&self, on_show: impl Fn() + Send + 'static) {
        let ev = self.show_event.0 as isize;
        if ev == 0 {
            return;
        }
        std::thread::Builder::new()
            .name("clatter-instance".into())
            .spawn(move || loop {
                let r = unsafe { WaitForSingleObject(HANDLE(ev as *mut _), INFINITE) };
                if r != WAIT_OBJECT_0 {
                    break;
                }
                on_show();
            })
            .ok();
    }
}
