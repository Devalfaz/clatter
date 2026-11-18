//! Clatter — satisfying mechanical keyboard sounds for Windows.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod hook;
mod icon;
mod keymap;
mod settings;
mod system;
mod tray;

slint::include_modules!();

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Minimal logger: `%APPDATA%\Clatter\clatter.log` (and stderr in debug builds).
struct Logger(Mutex<Option<std::fs::File>>);

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info && m.target().starts_with("clatter")
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!("[{secs}] {:<5} {}\n", r.level(), r.args());
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        if let Ok(mut f) = self.0.lock()
            && let Some(f) = f.as_mut()
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn flush(&self) {}
}

fn init_logging() {
    let dir = settings::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("clatter.log");
    // Start fresh when the log gets large.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    let logger = Box::leak(Box::new(Logger(Mutex::new(file))));
    let _ = log::set_logger(logger);
    log::set_max_level(log::LevelFilter::Info);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Developer helper: write every built-in sound to disk as folder packs.
    if let Some(i) = args.iter().position(|a| a == "--export-wavs") {
        let dir = args.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("clatter-sounds"));
        match audio::synth::export_wavs(&dir) {
            Ok(()) => println!("Wrote sounds to {}", dir.display()),
            Err(e) => eprintln!("Export failed: {e:#}"),
        }
        return;
    }
    let background = args.iter().any(|a| a == "--background");
    // Open Settings on a given page (0 General, 1 Sounds, 2 Auto-mute, 3 Sound packs, 4 About).
    let page = args
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<i32>().ok());

    let instance = match system::single_instance() {
        system::Instance::First(guard) => guard,
        system::Instance::AlreadyRunning => return, // the running copy opens its settings
    };

    init_logging();
    log::info!("Clatter {} starting", env!("CARGO_PKG_VERSION"));
    if let Err(e) = app::run(instance, background, page) {
        log::error!("fatal: {e:#}");
    }
}
