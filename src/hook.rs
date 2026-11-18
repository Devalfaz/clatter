//! Global keyboard hook and auto-mute ("sleep") detection.
//!
//! A dedicated thread installs a `WH_KEYBOARD_LL` hook. The hook callback does
//! the minimum: dedupe auto-repeat, apply filters (all atomics), and push a
//! key event into the audio ring. Windows silently removes low-level hooks
//! that take too long, so nothing here allocates, locks or does I/O.
//!
//! The same thread watches the foreground window (WinEvent hook + a 2 s
//! timer) to decide whether Clatter should currently be quiet: an excluded
//! app is in front, or a full-screen app / game / presentation is running.
//! Only key codes and timing are handled; what you type is never stored.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT, WM_TIMER, SetWindowsHookExW,
};

use crate::audio::engine::{KeyEvent, Waker, now_ms};
use crate::keymap::{self, NUM_CODES};
use crate::settings::Settings;
use crate::system;

const WM_REFRESH: u32 = WM_APP + 1;
/// A repeated key-down later than this after the last event for that key is
/// treated as a new press (its key-up was lost, e.g. to the secure desktop).
const REPEAT_WINDOW_MS: u32 = 1100;

/// State shared between the UI thread and the hook thread.
#[derive(Default)]
pub struct Shared {
    pub enabled: AtomicBool,
    key_up: AtomicBool,
    modifiers: AtomicBool,
    numpad: AtomicBool,
    function_keys: AtomicBool,
    ignore_injected: AtomicBool,
    mute_fullscreen: AtomicBool,
    exclusions: RwLock<Vec<String>>,
    suppressed: AtomicBool,
    thread_id: AtomicU32,
}

impl Shared {
    pub fn apply(&self, s: &Settings) {
        self.enabled.store(s.enabled, Relaxed);
        self.key_up.store(s.key_up, Relaxed);
        self.modifiers.store(s.modifiers, Relaxed);
        self.numpad.store(s.numpad, Relaxed);
        self.function_keys.store(s.function_keys, Relaxed);
        self.ignore_injected.store(s.ignore_injected, Relaxed);
        self.mute_fullscreen.store(s.mute_fullscreen, Relaxed);
        *self.exclusions.write().unwrap() = s.exclusions.clone();
        self.request_refresh();
    }

    fn request_refresh(&self) {
        let tid = self.thread_id.load(Relaxed);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_REFRESH, WPARAM(0), LPARAM(0));
            }
        }
    }
}

pub struct Hook {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Hook {
    fn drop(&mut self) {
        let tid = self.shared.thread_id.load(Relaxed);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct HookState {
    tx: rtrb::Producer<KeyEvent>,
    waker: Waker,
    shared: Arc<Shared>,
    down: [u64; NUM_CODES / 64],
    sounded: [u64; NUM_CODES / 64],
    last_seen: [u32; NUM_CODES],
    own_pid: u32,
    fg_pid: u32,
    fg_exe: String,
}

fn bit(set: &[u64], i: usize) -> bool {
    set[i / 64] & (1 << (i % 64)) != 0
}

fn set_bit(set: &mut [u64], i: usize, on: bool) {
    if on {
        set[i / 64] |= 1 << (i % 64);
    } else {
        set[i / 64] &= !(1 << (i % 64));
    }
}

impl HookState {
    fn on_key(&mut self, kb: &KBDLLHOOKSTRUCT) {
        let flags = kb.flags.0;
        let up = flags & LLKHF_UP.0 != 0;
        if kb.scanCode == 0 || kb.scanCode > 0xFF {
            return;
        }
        let code = keymap::code(kb.scanCode, flags & LLKHF_EXTENDED.0 != 0);
        // Windows wraps some navigation keys in fake Shift presses.
        if code == 0x12A || code == 0x136 {
            return;
        }
        let i = code as usize;
        let now = now_ms();
        let was_down = bit(&self.down, i);
        let last = std::mem::replace(&mut self.last_seen[i], now);

        if up {
            set_bit(&mut self.down, i, false);
            let sounded = bit(&self.sounded, i);
            set_bit(&mut self.sounded, i, false);
            if !sounded || !self.shared.key_up.load(Relaxed) {
                return;
            }
        } else {
            if was_down && now.wrapping_sub(last) < REPEAT_WINDOW_MS {
                return; // auto-repeat
            }
            set_bit(&mut self.down, i, true);
            let sh = &self.shared;
            let allowed = sh.enabled.load(Relaxed)
                && !sh.suppressed.load(Relaxed)
                && !(flags & LLKHF_INJECTED.0 != 0 && sh.ignore_injected.load(Relaxed))
                && (sh.modifiers.load(Relaxed) || !keymap::is_modifier(code))
                && (sh.numpad.load(Relaxed) || !keymap::is_numpad(code))
                && (sh.function_keys.load(Relaxed) || !keymap::is_function(code));
            if !allowed {
                return;
            }
            set_bit(&mut self.sounded, i, true);
        }
        if self.tx.push(KeyEvent { code, down: !up, t_ms: now }).is_ok() {
            self.waker.wake();
        }
    }

    fn refresh_suppression(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
        let mut quiet = false;
        if pid != 0 && pid != self.own_pid {
            if pid != self.fg_pid {
                self.fg_pid = pid;
                self.fg_exe = system::exe_name(pid).unwrap_or_default();
            }
            if !self.fg_exe.is_empty() && self.shared.exclusions.read().unwrap().contains(&self.fg_exe) {
                quiet = true;
            }
            if self.shared.mute_fullscreen.load(Relaxed)
                && let Ok(state) = unsafe { SHQueryUserNotificationState() }
                && (state == QUNS_BUSY || state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
            {
                quiet = true;
            }
        }
        self.shared.suppressed.store(quiet, Relaxed);
    }
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        STATE.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut()
                && let Some(st) = s.as_mut()
            {
                st.on_key(kb);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn refresh() {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && let Some(st) = s.as_mut()
        {
            st.refresh_suppression();
        }
    });
}

unsafe extern "system" fn foreground_proc(
    _: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _idobject: i32,
    _idchild: i32,
    _thread: u32,
    _time: u32,
) {
    refresh();
}

pub fn start(shared: Arc<Shared>, tx: rtrb::Producer<KeyEvent>, waker: Waker) -> Hook {
    let sh = shared.clone();
    let thread = std::thread::Builder::new()
        .name("clatter-hook".into())
        .spawn(move || unsafe {
            // Make sure this thread has a message queue before publishing its id.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            STATE.with(|s| {
                *s.borrow_mut() = Some(HookState {
                    tx,
                    waker,
                    shared: sh.clone(),
                    down: [0; NUM_CODES / 64],
                    sounded: [0; NUM_CODES / 64],
                    last_seen: [0; NUM_CODES],
                    own_pid: GetCurrentProcessId(),
                    fg_pid: 0,
                    fg_exe: String::new(),
                })
            });
            sh.thread_id.store(GetCurrentThreadId(), Relaxed);

            let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
            let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) {
                Ok(h) => Some(h),
                Err(e) => {
                    log::error!("could not install keyboard hook: {e}");
                    None
                }
            };
            let win_hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(foreground_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            SetTimer(None, 0, 2000, None);
            refresh();

            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_TIMER | WM_REFRESH => refresh(),
                    _ => {
                        DispatchMessageW(&msg);
                    }
                }
            }
            if let Some(h) = hook {
                let _ = UnhookWindowsHookEx(h);
            }
            if !win_hook.is_invalid() {
                let _ = UnhookWinEvent(win_hook);
            }
            sh.thread_id.store(0, Relaxed);
        })
        .expect("spawn hook thread");
    Hook { shared, thread: Some(thread) }
}
