//! Application state and glue between settings, audio, hook, tray and UI.
//!
//! Everything here lives on the main (UI) thread in a thread-local. Events
//! from the tray, the hotkey, background loaders and the single-instance
//! listener are posted onto the Slint event loop and handled via [`with_app`].

use std::cell::RefCell;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::audio::bank::Bank;
use crate::audio::engine::{self, Audio};
use crate::audio::packs::{self, PackInfo};
use crate::audio::synth;
use crate::hook::{self, Hook};
use crate::icon::{self, IconStyle};
use crate::settings::Settings;
use crate::system::{self, InstanceGuard};
use crate::tray::{self, Tray};
use crate::{SettingsWindow, SwitchItem};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app state. A re-entrant call (which shouldn't happen) is
/// deferred to the next event-loop turn instead of panicking.
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    let mut f = Some(f);
    let done = APP.with(|a| match a.try_borrow_mut() {
        Ok(mut guard) => {
            if let (Some(app), Some(f)) = (guard.as_mut(), f.take()) {
                f(app);
            }
            true
        }
        Err(_) => false,
    });
    if !done && let Some(f) = f {
        slint::Timer::single_shot(Duration::ZERO, move || with_app(f));
    }
}

/// Runs `f` synchronously if the app state is available.
fn try_with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

/// Posts `f` to the UI thread (callable from any thread).
pub fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

pub struct App {
    settings: Settings,
    audio: Audio,
    shared: Arc<hook::Shared>,
    _hook: Hook,
    tray: Option<Tray>,
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    window: Option<SettingsWindow>,
    packs: Vec<PackInfo>,
    devices: Vec<String>,
    save_timer: slint::Timer,
    loading_switch: Option<String>,
    _instance: InstanceGuard,
}

pub fn run(instance: InstanceGuard, background: bool) -> anyhow::Result<()> {
    let (settings, first_run) = Settings::load();
    let (audio, key_tx, waker) = Audio::start(settings.output_device.clone());
    let shared = Arc::new(hook::Shared::default());
    shared.apply(&settings);
    let hook = hook::start(shared.clone(), key_tx, waker);

    let hotkeys = GlobalHotKeyManager::new().map_err(|e| log::warn!("global hotkeys unavailable: {e}")).ok();

    instance.listen(|| post(|app| app.show_settings()));

    let app = App {
        settings,
        audio,
        shared,
        _hook: hook,
        tray: None,
        hotkeys,
        hotkey: None,
        window: None,
        packs: packs::list_packs(),
        devices: Vec::new(),
        save_timer: slint::Timer::default(),
        loading_switch: None,
        _instance: instance,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));

    tray::set_menu_handler(|id| post(move |app| app.on_menu(&id)));
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            post(|app| app.toggle_enabled());
        }
    }));

    with_app(move |app| {
        app.startup();
        if !background || first_run {
            app.show_settings();
        }
    });

    let ticker = slint::Timer::default();
    ticker.start(slint::TimerMode::Repeated, Duration::from_secs(3), || with_app(|app| app.tick()));

    slint::run_event_loop_until_quit()?;

    // Tear down in order: save, remove tray, stop hook, stop audio.
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().take() {
            app.settings.save();
            drop(app);
        }
    });
    Ok(())
}

fn app_icon() -> slint::Image {
    const SIZE: u32 = 128;
    let rgba = icon::render(SIZE, IconStyle::Color);
    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&rgba, SIZE, SIZE))
}

impl App {
    fn startup(&mut self) {
        let switches = self.switch_list();
        match Tray::new(&self.settings, &switches, system::taskbar_is_light()) {
            Ok(t) => self.tray = Some(t),
            Err(e) => log::error!("could not create tray icon: {e}"),
        }
        self.register_hotkey();
        if self.settings.launch_at_login {
            // Keep the registered path current if the exe moved.
            let _ = system::set_launch_at_login(true);
        }
        self.load_switch(self.settings.switch_id.clone());
    }

    /// (id, name, builtin) for every available switch.
    fn switch_list(&self) -> Vec<(String, String, bool)> {
        synth::PROFILES
            .iter()
            .map(|p| (p.id.to_owned(), p.name.to_owned(), true))
            .chain(self.packs.iter().map(|p| (p.id.clone(), p.name.clone(), false)))
            .collect()
    }

    /// Builds the bank for a switch on a worker thread, then installs it.
    fn load_switch(&mut self, id: String) {
        if self.audio.current_bank_id() == Some(id.as_str()) || self.loading_switch.as_deref() == Some(&id) {
            return;
        }
        self.loading_switch = Some(id.clone());
        std::thread::spawn(move || {
            let result = build_bank(&id);
            post(move |app| {
                if app.loading_switch.as_deref() == Some(id.as_str()) {
                    app.loading_switch = None;
                }
                match result {
                    Ok(bank) if app.settings.switch_id == id => app.audio.set_bank(bank),
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("could not load switch {id}: {e:#}");
                        if app.settings.switch_id == id && id != synth::DEFAULT_PROFILE {
                            app.settings.switch_id = synth::DEFAULT_PROFILE.to_owned();
                            app.settings_changed(true);
                        }
                    }
                }
            });
        });
    }

    fn register_hotkey(&mut self) {
        let Some(mgr) = &self.hotkeys else { return };
        if let Some(old) = self.hotkey.take() {
            let _ = mgr.unregister(old);
        }
        match HotKey::from_str(&self.settings.hotkey.replace("Win", "Super")) {
            Ok(hk) => match mgr.register(hk) {
                Ok(()) => self.hotkey = Some(hk),
                Err(e) => log::warn!("could not register {}: {e}", self.settings.hotkey),
            },
            Err(e) => log::warn!("invalid hotkey {}: {e}", self.settings.hotkey),
        }
    }

    /// Switches to a newly recorded shortcut. Returns false if it can't be registered
    /// (e.g. another app owns it), so the user can try another one.
    fn set_hotkey(&mut self, combo: String) -> bool {
        let Ok(hk) = HotKey::from_str(&combo.replace("Win", "Super")) else { return false };
        let Some(mgr) = &self.hotkeys else { return false };
        if let Some(old) = self.hotkey.take() {
            let _ = mgr.unregister(old);
        }
        if mgr.register(hk).is_err() {
            return false;
        }
        self.hotkey = Some(hk);
        self.settings.hotkey = combo;
        self.settings_changed(true);
        true
    }

    fn unregister_hotkey(&mut self) {
        if let (Some(mgr), Some(hk)) = (&self.hotkeys, self.hotkey.take()) {
            let _ = mgr.unregister(hk);
        }
    }

    /// Applies `self.settings` everywhere after a change.
    fn settings_changed(&mut self, sync_window: bool) {
        let s = &self.settings;
        self.shared.apply(s);
        self.audio.params().set_volume(s.volume);
        self.audio.params().set_pitch_range(s.pitch_range);
        self.audio.params().set_spatial(s.spatial_width);
        self.audio.set_device(s.output_device.clone());
        self.load_switch(s.switch_id.clone());
        let switches = self.switch_list();
        if let Some(t) = &mut self.tray {
            t.update(&self.settings, &switches);
        }
        if sync_window {
            self.sync_window();
        }
        // Debounced save (sliders fire many changes).
        let settings = self.settings.clone();
        self.save_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(600), move || settings.save());
    }

    fn tick(&mut self) {
        if let Some(t) = &mut self.tray {
            t.set_dark(system::taskbar_is_light());
        }
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            tray::ID_SOUND => self.toggle_enabled(),
            tray::ID_SETTINGS => self.show_settings(),
            tray::ID_QUIT => {
                let _ = slint::quit_event_loop();
            }
            _ => {
                if let Some(i) = id.strip_prefix(tray::ID_VOLUME_PREFIX).and_then(|i| i.parse::<usize>().ok()) {
                    if let Some((_, v)) = tray::VOLUME_PRESETS.get(i) {
                        self.settings.volume = *v;
                        self.settings_changed(true);
                    }
                } else if let Some(sw) = id.strip_prefix(tray::ID_SWITCH_PREFIX) {
                    self.settings.switch_id = sw.to_owned();
                    self.settings_changed(true);
                }
            }
        }
    }

    fn toggle_enabled(&mut self) {
        self.settings.enabled = !self.settings.enabled;
        self.settings_changed(true);
        // A short confirmation sound when turning on.
        if self.settings.enabled {
            self.audio.play(0x01C, true);
        }
    }

    // ------------------------------------------------------------------ UI

    pub fn show_settings(&mut self) {
        if self.window.is_none() {
            match self.create_window() {
                Ok(w) => self.window = Some(w),
                Err(e) => {
                    log::error!("could not open settings window: {e}");
                    return;
                }
            }
        }
        self.sync_lists();
        self.sync_window();
        if let Some(w) = &self.window {
            let _ = w.show();
            bring_to_front(w);
        }
    }

    fn create_window(&mut self) -> Result<SettingsWindow, slint::PlatformError> {
        let w = SettingsWindow::new()?;
        w.set_app_icon(app_icon());
        w.set_version(env!("CARGO_PKG_VERSION").into());

        w.on_changed(|| with_app(|app| app.read_window()));
        w.on_select_switch(|id| {
            let id = id.to_string();
            with_app(move |app| {
                app.settings.switch_id = id;
                app.settings_changed(true);
            })
        });
        w.on_preview_switch(|id| {
            let id = id.to_string();
            with_app(move |app| app.preview(id))
        });
        w.on_hotkey_record_start(|| with_app(|app| app.unregister_hotkey()));
        w.on_hotkey_record_cancel(|| with_app(|app| app.register_hotkey()));
        w.on_hotkey_captured(|text: SharedString, ctrl, alt, shift, meta| {
            let Some(combo) = hotkey_string(&text, ctrl, alt, shift, meta) else { return false };
            try_with_app(|app| app.set_hotkey(combo)).unwrap_or(false)
        });
        w.on_refresh_running_apps(|| with_app(|app| app.refresh_running_apps()));
        w.on_add_exclusion(|name| {
            let name = name.to_string();
            with_app(move |app| app.add_exclusion(name))
        });
        w.on_remove_exclusion(|i| {
            with_app(move |app| {
                if (i as usize) < app.settings.exclusions.len() {
                    app.settings.exclusions.remove(i as usize);
                    app.settings_changed(true);
                }
            })
        });
        w.on_browse_exclusion(|| {
            std::thread::spawn(|| {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose an app to mute Clatter in")
                    .add_filter("Programs", &["exe"])
                    .pick_file()
                    && let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase())
                {
                    post(move |app| app.add_exclusion(name));
                }
            });
        });
        w.on_import_pack_folder(|| {
            std::thread::spawn(|| {
                if let Some(dir) = rfd::FileDialog::new().set_title("Choose a sound pack folder").pick_folder() {
                    import_pack(move || packs::import_folder(&dir));
                }
            });
        });
        w.on_import_pack_zip(|| {
            std::thread::spawn(|| {
                if let Some(file) = rfd::FileDialog::new()
                    .set_title("Choose a sound pack .zip")
                    .add_filter("Zip archives", &["zip"])
                    .pick_file()
                {
                    import_pack(move || packs::import_zip(&file));
                }
            });
        });
        w.on_open_packs_folder(|| {
            let dir = packs::packs_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
        });
        w.on_remove_pack(|id| {
            let id = id.to_string();
            with_app(move |app| app.remove_pack(&id))
        });

        w.window().on_close_requested(|| {
            with_app(|app| {
                if app.hotkey.is_none() {
                    app.register_hotkey(); // was recording a shortcut
                }
                // Free the window's resources; it's recreated on demand.
                slint::Timer::single_shot(Duration::from_millis(50), || with_app(|app| app.window = None));
            });
            slint::CloseRequestResponse::HideWindow
        });
        Ok(w)
    }

    /// Pushes settings and lists into the window's properties.
    fn sync_window(&mut self) {
        let Some(w) = &self.window else { return };
        let s = &self.settings;
        w.set_enabled(s.enabled);
        w.set_volume(s.volume * 100.0);
        w.set_hotkey_text(s.hotkey.clone().into());
        w.set_launch_at_login(s.launch_at_login);
        w.set_show_tray_icon(s.show_tray_icon);
        w.set_pitch(s.pitch_range / 2.0 * 100.0);
        w.set_spatial(s.spatial_width * 100.0);
        w.set_key_up(s.key_up);
        w.set_modifiers(s.modifiers);
        w.set_numpad(s.numpad);
        w.set_function_keys(s.function_keys);
        w.set_mute_fullscreen(s.mute_fullscreen);
        w.set_current_switch(s.switch_id.clone().into());

        let index = s
            .output_device
            .as_ref()
            .and_then(|d| self.devices.iter().position(|x| x == d))
            .map(|i| i as i32 + 1)
            .unwrap_or(0);
        w.set_output_device_index(index);
        w.set_exclusions(ModelRc::new(VecModel::from(
            s.exclusions.iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
    }

    /// Refreshes the window's lists (devices, switches). Called when opening the
    /// window and when packs change, not on every settings change.
    fn sync_lists(&mut self) {
        let Some(w) = &self.window else { return };
        self.devices = engine::output_device_names();
        let mut device_names: Vec<SharedString> = vec!["System default".into()];
        device_names.extend(self.devices.iter().map(SharedString::from));
        w.set_output_devices(ModelRc::new(VecModel::from(device_names)));

        let items: Vec<SwitchItem> = synth::PROFILES
            .iter()
            .map(|p| SwitchItem {
                id: p.id.into(),
                name: p.name.into(),
                description: p.description.into(),
                builtin: true,
            })
            .chain(self.packs.iter().map(|p| SwitchItem {
                id: p.id.clone().into(),
                name: p.name.clone().into(),
                description: match p.kind {
                    packs::PackKind::Mechvibes => "Imported Mechvibes pack".into(),
                    packs::PackKind::Folder => "Imported folder pack".into(),
                },
                builtin: false,
            }))
            .collect();
        w.set_switches(ModelRc::new(VecModel::from(items)));
    }

    /// Reads every editable property back from the window.
    fn read_window(&mut self) {
        let Some(w) = &self.window else { return };
        let s = &mut self.settings;
        s.enabled = w.get_enabled();
        s.volume = (w.get_volume() / 100.0).clamp(0.0, 1.0);
        s.launch_at_login_changed(w.get_launch_at_login());
        s.show_tray_icon = w.get_show_tray_icon();
        s.pitch_range = (w.get_pitch() / 100.0 * 2.0).clamp(0.0, 2.0);
        s.spatial_width = (w.get_spatial() / 100.0).clamp(0.0, 1.0);
        s.key_up = w.get_key_up();
        s.modifiers = w.get_modifiers();
        s.numpad = w.get_numpad();
        s.function_keys = w.get_function_keys();
        s.mute_fullscreen = w.get_mute_fullscreen();
        let idx = w.get_output_device_index();
        s.output_device = if idx <= 0 { None } else { self.devices.get(idx as usize - 1).cloned() };
        self.settings_changed(false);
    }

    fn preview(&mut self, id: String) {
        if id != self.settings.switch_id {
            self.settings.switch_id = id;
            self.settings_changed(true);
        }
        // Play a little phrase once the bank is ready.
        let phrase: &[(u64, u16, bool)] = &[
            (0, 0x023, true),
            (70, 0x023, false),
            (110, 0x012, true),
            (180, 0x012, false),
            (220, 0x026, true),
            (290, 0x026, false),
            (330, 0x026, true),
            (400, 0x026, false),
            (440, 0x018, true),
            (510, 0x018, false),
            (600, 0x039, true),
            (700, 0x039, false),
            (800, 0x01C, true),
            (900, 0x01C, false),
        ];
        let delay = if self.loading_switch.is_some() { 250 } else { 0 };
        for &(t, code, down) in phrase {
            slint::Timer::single_shot(Duration::from_millis(delay + t), move || {
                with_app(move |app| app.audio.play(code, down));
            });
        }
    }

    fn refresh_running_apps(&mut self) {
        let Some(w) = &self.window else { return };
        let apps: Vec<SharedString> = system::running_apps()
            .into_iter()
            .filter(|a| !self.settings.exclusions.contains(a))
            .map(SharedString::from)
            .collect();
        w.set_running_apps(ModelRc::new(VecModel::from(apps)));
        w.set_running_app_index(0);
    }

    fn add_exclusion(&mut self, name: String) {
        let name = name.trim().to_lowercase();
        if name.is_empty() || self.settings.exclusions.contains(&name) {
            return;
        }
        self.settings.exclusions.push(name);
        self.settings.exclusions.sort();
        self.settings_changed(true);
        self.refresh_running_apps();
    }

    fn reload_packs(&mut self) {
        self.packs = packs::list_packs();
        let switches = self.switch_list();
        if let Some(t) = &mut self.tray {
            t.set_switches(&switches, &self.settings.switch_id);
        }
        self.sync_lists();
        self.sync_window();
    }

    fn remove_pack(&mut self, id: &str) {
        let result = packs::remove_pack(id);
        if let Some(w) = &self.window {
            w.set_pack_status(match &result {
                Ok(()) => "Pack removed.".into(),
                Err(e) => format!("Could not remove the pack: {e}").into(),
            });
        }
        if self.settings.switch_id == id {
            self.settings.switch_id = synth::DEFAULT_PROFILE.to_owned();
            self.settings_changed(false);
        }
        self.reload_packs();
    }
}

impl Settings {
    fn launch_at_login_changed(&mut self, enabled: bool) {
        if self.launch_at_login != enabled {
            match system::set_launch_at_login(enabled) {
                Ok(()) => self.launch_at_login = enabled,
                Err(e) => log::warn!("could not change launch at login: {e}"),
            }
        }
    }
}

fn build_bank(id: &str) -> anyhow::Result<Bank> {
    if let Some(p) = synth::profile(id) {
        return Ok(synth::build_bank(p));
    }
    let info = packs::find_pack(id).ok_or_else(|| anyhow::anyhow!("unknown switch {id}"))?;
    packs::load_bank(&info)
}

/// Runs an import on the current (worker) thread and reports back to the UI.
fn import_pack(f: impl FnOnce() -> anyhow::Result<PackInfo>) {
    post(|app| {
        if let Some(w) = &app.window {
            w.set_pack_status("Importing…".into());
        }
    });
    let result = f();
    post(move |app| {
        let status = match &result {
            Ok(info) => format!("Imported “{}”. It's now your active switch.", info.name),
            Err(e) => format!("Couldn't import that pack: {e:#}"),
        };
        if let Some(w) = &app.window {
            w.set_pack_status(status.into());
        }
        if let Ok(info) = result {
            app.reload_packs();
            app.settings.switch_id = info.id;
            app.settings_changed(true);
        }
    });
}

/// Formats a captured key combination, e.g. "Ctrl+Alt+K". `None` if it isn't usable.
fn hotkey_string(text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let key = match c {
        'a'..='z' | 'A'..='Z' | '0'..='9' => c.to_ascii_uppercase().to_string(),
        ' ' => "Space".to_owned(),
        '\u{F704}'..='\u{F70F}' => format!("F{}", c as u32 - 0xF704 + 1),
        _ => return None,
    };
    let is_fkey = key.starts_with('F') && key.len() > 1;
    if !(ctrl || alt || meta || is_fkey) {
        return None;
    }
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl".to_owned());
    }
    if alt {
        parts.push("Alt".to_owned());
    }
    if shift {
        parts.push("Shift".to_owned());
    }
    if meta {
        parts.push("Win".to_owned());
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn bring_to_front(w: &SettingsWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SetForegroundWindow, ShowWindow};
    let handle = w.window().window_handle();
    if let Ok(h) = handle.window_handle()
        && let RawWindowHandle::Win32(win) = h.as_raw()
    {
        let hwnd = HWND(win.hwnd.get() as *mut _);
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::hotkey_string;

    #[test]
    fn hotkeys() {
        assert_eq!(hotkey_string("k", true, true, false, false).as_deref(), Some("Ctrl+Alt+K"));
        assert_eq!(hotkey_string("K", true, false, true, false).as_deref(), Some("Ctrl+Shift+K"));
        assert_eq!(hotkey_string("\u{F70B}", false, false, false, false).as_deref(), Some("F8"));
        assert_eq!(hotkey_string("k", false, false, true, false), None); // needs a real modifier
        assert_eq!(hotkey_string("\u{0011}", true, false, false, false), None); // modifier alone
        assert_eq!(hotkey_string(" ", false, false, false, true).as_deref(), Some("Win+Space"));
    }
}
