# Clatter

Satisfying mechanical keyboard sounds for Windows — a tray app in the spirit of Klack for Mac.

- Separate key-down and key-up sounds, randomized pitch and level, several variants per key
- Spatial audio: keys pan left/right by where they sit on the keyboard
- Six original switch profiles synthesized in code (Obsidian, Marshmallow, Glass, Click Jacket, Typewriter, Chiclet)
- Import real recordings: Mechvibes packs (folder or .zip with `config.json`) and Clatter folder packs
- Tray menu: sound on/off, Soft/Balanced/Loud, switch picker, settings
- Global toggle shortcut (default **Ctrl+Alt+K**), output device choice, launch at login, hide tray icon
- Auto-mute in full-screen apps/games/presentations and in apps you choose
- Ignores auto-repeat and synthesized keystrokes; never records what you type

## Build

Requires Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools.

```
cargo build --release
```

The executable is `target\release\clatter.exe` (single file). If the project lives in a very deep folder,
the MSVC linker can hit Windows' 260-character path limit; set `CARGO_TARGET_DIR` to a short path.

Useful commands:

- `cargo test` — unit tests (key map, synth levels, pack parsing, settings)
- `clatter.exe --export-wavs <dir>` — write every built-in sound as WAV folder packs (for auditioning or editing)
- `clatter.exe --background` — start in the tray without opening settings (used by launch at login)

Settings, packs and the log live in `%APPDATA%\Clatter`.

## Folder pack format

A folder with `pack.json` (`{"name": "My Switches"}`) and audio files named
`<group>_<down|up>_<n>.<wav|ogg|mp3|flac>`, where group is `regular`, `wide`, `backspace`, `enter` or `space`.
Missing groups fall back to `regular`; up sounds are optional. `--export-wavs` produces this layout.

## Layout

| File | Purpose |
| --- | --- |
| `src/hook.rs` | Low-level keyboard hook thread, auto-repeat filtering, auto-mute detection |
| `src/audio/engine.rs` | cpal output, real-time mixer, idle pause so Windows can sleep |
| `src/audio/synth.rs` | Procedural switch sounds |
| `src/audio/packs.rs` | Mechvibes / folder pack loading and import |
| `src/tray.rs`, `src/app.rs` | Tray menu, hotkey, settings glue |
| `ui/app.slint` | Settings window (Slint, Fluent style) |

The settings window uses [Slint](https://slint.dev) under its royalty-free license (attribution on the About page).

## License

MIT. See [LICENSE](LICENSE).
