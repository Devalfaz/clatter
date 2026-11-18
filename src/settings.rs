//! Persistent user settings (`%APPDATA%\Clatter\settings.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const VOLUME_SOFT: f32 = 0.35;
pub const VOLUME_BALANCED: f32 = 0.6;
pub const VOLUME_LOUD: f32 = 1.0;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+K";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// 0..1 slider value (mapped to gain by the engine).
    pub volume: f32,
    pub switch_id: String,
    /// Random pitch variation, ± semitones.
    pub pitch_range: f32,
    /// 0 = mono, 1 = full stereo spread by key position.
    pub spatial_width: f32,
    pub key_up: bool,
    pub modifiers: bool,
    pub numpad: bool,
    pub function_keys: bool,
    /// Ignore keystrokes synthesized by other software (text expanders, macros).
    pub ignore_injected: bool,
    /// Output device name; `None` follows the Windows default.
    pub output_device: Option<String>,
    pub hotkey: String,
    pub launch_at_login: bool,
    pub show_tray_icon: bool,
    pub mute_fullscreen: bool,
    /// Lower-case executable names (e.g. `zoom.exe`) where Clatter stays quiet.
    pub exclusions: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            volume: VOLUME_BALANCED,
            switch_id: crate::audio::synth::DEFAULT_PROFILE.to_owned(),
            pitch_range: 0.8,
            spatial_width: 0.6,
            key_up: true,
            modifiers: true,
            numpad: true,
            function_keys: true,
            ignore_injected: true,
            output_device: None,
            hotkey: DEFAULT_HOTKEY.to_owned(),
            launch_at_login: false,
            show_tray_icon: true,
            mute_fullscreen: true,
            exclusions: Vec::new(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Clatter")
}

fn path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    /// Loads settings; the flag is true when no settings file existed yet (first run).
    pub fn load() -> (Settings, bool) {
        match std::fs::read_to_string(path()) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => (s.sanitized(), false),
                Err(e) => {
                    log::warn!("settings.json is invalid ({e}); using defaults");
                    (Settings::default(), false)
                }
            },
            Err(_) => (Settings::default(), true),
        }
    }

    pub fn save(&self) {
        let dir = data_dir();
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let tmp = dir.join("settings.json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
            std::fs::rename(&tmp, path())
        })();
        if let Err(e) = result {
            log::warn!("could not save settings: {e}");
        }
    }

    fn sanitized(mut self) -> Settings {
        self.volume = self.volume.clamp(0.0, 1.0);
        self.pitch_range = self.pitch_range.clamp(0.0, 3.0);
        self.spatial_width = self.spatial_width.clamp(0.0, 1.0);
        for e in &mut self.exclusions {
            *e = e.trim().to_lowercase();
        }
        self.exclusions.retain(|e| !e.is_empty());
        self.exclusions.sort();
        self.exclusions.dedup();
        if self.hotkey.trim().is_empty() {
            self.hotkey = DEFAULT_HOTKEY.to_owned();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            volume: 0.42,
            exclusions: vec!["zoom.exe".into()],
            output_device: Some("Speakers (Realtek)".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"volume": 0.9}"#).unwrap();
        assert_eq!(s.volume, 0.9);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert!(s.enabled);
    }

    #[test]
    fn sanitizes() {
        let s: Settings =
            serde_json::from_str(r#"{"volume": 7, "exclusions": [" Zoom.EXE ", "zoom.exe", ""]}"#).unwrap();
        let s = s.sanitized();
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.exclusions, vec!["zoom.exe".to_string()]);
    }
}
