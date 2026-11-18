//! System tray icon and menu (Klack's menu-bar menu, Windows style).

use tray_icon::menu::accelerator::Accelerator;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::icon::{self, IconStyle};
use crate::settings::{Settings, VOLUME_BALANCED, VOLUME_LOUD, VOLUME_SOFT};

pub const ID_SOUND: &str = "sound";
pub const ID_SETTINGS: &str = "settings";
pub const ID_QUIT: &str = "quit";
pub const ID_VOLUME_PREFIX: &str = "volume:";
pub const ID_SWITCH_PREFIX: &str = "switch:";

pub const VOLUME_PRESETS: [(&str, f32); 3] =
    [("Soft", VOLUME_SOFT), ("Balanced", VOLUME_BALANCED), ("Loud", VOLUME_LOUD)];

pub struct Tray {
    icon: TrayIcon,
    sound: CheckMenuItem,
    volume: Vec<CheckMenuItem>,
    switches: Submenu,
    switch_items: Vec<(String, CheckMenuItem)>,
    switch_extras: Vec<PredefinedMenuItem>,
    dark: bool,
    active: bool,
}

fn tray_icon(dark: bool, active: bool) -> Option<Icon> {
    const SIZE: u32 = 32;
    Icon::from_rgba(icon::render(SIZE, IconStyle::Tray { dark, active }), SIZE, SIZE).ok()
}

fn accelerator(hotkey: &str) -> Option<Accelerator> {
    hotkey.replace("Win", "Super").parse().ok()
}

impl Tray {
    /// `switches` is the list of (id, name, builtin).
    pub fn new(settings: &Settings, switches: &[(String, String, bool)], dark: bool) -> anyhow::Result<Tray> {
        let sound = CheckMenuItem::with_id(ID_SOUND, "Keyboard sounds", true, settings.enabled, accelerator(&settings.hotkey));
        let volume_menu = Submenu::new("Volume", true);
        let volume: Vec<CheckMenuItem> = VOLUME_PRESETS
            .iter()
            .enumerate()
            .map(|(i, (name, v))| {
                CheckMenuItem::with_id(
                    format!("{ID_VOLUME_PREFIX}{i}"),
                    *name,
                    true,
                    (settings.volume - v).abs() < 0.005,
                    None,
                )
            })
            .collect();
        for item in &volume {
            volume_menu.append(item)?;
        }
        let switches_menu = Submenu::new("Switches", true);

        let menu = Menu::new();
        menu.append_items(&[
            &sound,
            &PredefinedMenuItem::separator(),
            &volume_menu,
            &switches_menu,
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(ID_SETTINGS, "Clatter settings…", true, None),
            &MenuItem::with_id(ID_QUIT, "Quit Clatter", true, None),
        ])?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_tooltip("Clatter")
            .with_icon(tray_icon(dark, settings.enabled).ok_or_else(|| anyhow::anyhow!("icon"))?)
            .build()?;

        let mut tray = Tray {
            icon,
            sound,
            volume,
            switches: switches_menu,
            switch_items: Vec::new(),
            switch_extras: Vec::new(),
            dark,
            active: settings.enabled,
        };
        tray.set_switches(switches, &settings.switch_id);
        tray.update(settings, switches);
        let _ = tray.icon.set_visible(settings.show_tray_icon);
        Ok(tray)
    }

    pub fn set_switches(&mut self, switches: &[(String, String, bool)], current: &str) {
        for (_, item) in self.switch_items.drain(..) {
            let _ = self.switches.remove(&item);
        }
        for sep in self.switch_extras.drain(..) {
            let _ = self.switches.remove(&sep);
        }
        let mut seen_pack = false;
        for (id, name, builtin) in switches {
            if !builtin && !seen_pack {
                seen_pack = true;
                let sep = PredefinedMenuItem::separator();
                let _ = self.switches.append(&sep);
                self.switch_extras.push(sep);
            }
            let item = CheckMenuItem::with_id(format!("{ID_SWITCH_PREFIX}{id}"), name, true, id == current, None);
            let _ = self.switches.append(&item);
            self.switch_items.push((id.clone(), item));
        }
    }

    /// Syncs check marks, tooltip and icon with the settings.
    pub fn update(&mut self, settings: &Settings, switches: &[(String, String, bool)]) {
        self.sound.set_checked(settings.enabled);
        self.sound.set_accelerator(accelerator(&settings.hotkey)).ok();
        for (item, (_, v)) in self.volume.iter().zip(VOLUME_PRESETS) {
            item.set_checked((settings.volume - v).abs() < 0.005);
        }
        for (id, item) in &self.switch_items {
            item.set_checked(*id == settings.switch_id);
        }
        let name = switches
            .iter()
            .find(|(id, _, _)| *id == settings.switch_id)
            .map(|(_, n, _)| n.as_str())
            .unwrap_or("");
        let tip = if settings.enabled { format!("Clatter · {name}") } else { "Clatter · Off".to_owned() };
        let _ = self.icon.set_tooltip(Some(tip));
        if self.active != settings.enabled {
            self.active = settings.enabled;
            let _ = self.icon.set_icon(tray_icon(self.dark, self.active));
        }
        let _ = self.icon.set_visible(settings.show_tray_icon);
    }

    pub fn set_dark(&mut self, dark: bool) {
        if self.dark != dark {
            self.dark = dark;
            let _ = self.icon.set_icon(tray_icon(self.dark, self.active));
        }
    }
}

pub fn set_menu_handler(f: impl Fn(String) + Send + Sync + 'static) {
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| f(e.id().0.clone())));
}
