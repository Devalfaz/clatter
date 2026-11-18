//! Sound packs on disk: Mechvibes packs and Clatter folder packs.
//!
//! * **Mechvibes**: a folder with `config.json`. `key_define_type: "single"`
//!   means one audio file with `[start_ms, length_ms]` per key; `"multi"`
//!   means one file per key. Keys are libuiohook key codes.
//! * **Folder pack**: a folder with `pack.json` (`{"name": "..."}`) and files
//!   named `<group>_<down|up>_<n>.<wav|ogg|mp3|flac>`, where group is one of
//!   regular, wide, backspace, enter, space. Missing groups fall back to regular.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::bank::{Bank, NO_SLOT, Sample};
use super::decode::{decode_file, slice_ms};
use crate::keymap::{self, KeyGroup, NUM_CODES};

pub const PACK_PREFIX: &str = "pack:";

#[derive(Clone, Debug, PartialEq)]
pub enum PackKind {
    Mechvibes,
    Folder,
}

#[derive(Clone, Debug)]
pub struct PackInfo {
    /// Switch id, `pack:<folder name>`.
    pub id: String,
    pub name: String,
    pub kind: PackKind,
    pub dir: PathBuf,
}

pub fn packs_dir() -> PathBuf {
    crate::settings::data_dir().join("packs")
}

fn detect(dir: &Path) -> Option<PackKind> {
    if dir.join("config.json").is_file() {
        Some(PackKind::Mechvibes)
    } else if dir.join("pack.json").is_file() {
        Some(PackKind::Folder)
    } else {
        None
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Some packs are saved with a UTF-8 BOM.
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
}

pub fn pack_info(dir: &Path) -> Option<PackInfo> {
    let kind = detect(dir)?;
    let folder = dir.file_name()?.to_string_lossy().to_string();
    let manifest = match kind {
        PackKind::Mechvibes => dir.join("config.json"),
        PackKind::Folder => dir.join("pack.json"),
    };
    let name = read_json(&manifest)
        .ok()
        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| folder.clone());
    Some(PackInfo { id: format!("{PACK_PREFIX}{folder}"), name, kind, dir: dir.to_path_buf() })
}

/// All installed packs, sorted by name.
pub fn list_packs() -> Vec<PackInfo> {
    let mut packs: Vec<PackInfo> = fs::read_dir(packs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| pack_info(&e.path()))
        .collect();
    packs.sort_by_key(|p| p.name.to_lowercase());
    packs
}

pub fn find_pack(id: &str) -> Option<PackInfo> {
    let folder = id.strip_prefix(PACK_PREFIX)?;
    pack_info(&packs_dir().join(folder))
}

pub fn load_bank(info: &PackInfo) -> Result<Bank> {
    let bank = match info.kind {
        PackKind::Mechvibes => load_mechvibes(&info.dir, &info.id)?,
        PackKind::Folder => load_folder(&info.dir, &info.id)?,
    };
    if bank.is_empty() {
        bail!("the pack contains no sounds");
    }
    Ok(bank)
}

fn safe_join(dir: &Path, name: &str) -> Result<PathBuf> {
    let rel = Path::new(name);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        bail!("invalid file name in pack: {name}");
    }
    Ok(dir.join(rel))
}

pub fn load_mechvibes(dir: &Path, id: &str) -> Result<Bank> {
    let config = read_json(&dir.join("config.json"))?;
    let defines = config
        .get("defines")
        .and_then(|d| d.as_object())
        .ok_or_else(|| anyhow!("config.json has no \"defines\""))?;
    let kind = config.get("key_define_type").and_then(|k| k.as_str()).unwrap_or("single");

    let mut bank = Bank::new(id);
    let mut fallback = NO_SLOT;

    let assign = |bank: &mut Bank, key: &str, slot: u16, fallback: &mut u16| {
        let (key, up) = match key.strip_suffix("-up") {
            Some(k) => (k, true),
            None => (key, false),
        };
        let Some(code) = key.trim().parse::<u32>().ok().and_then(keymap::from_mechvibes) else {
            return;
        };
        if up {
            bank.up[code as usize] = slot;
        } else {
            bank.down[code as usize] = slot;
            // Prefer the "A" key as the sound for keys the pack doesn't define.
            if code == 0x1E || *fallback == NO_SLOT {
                *fallback = slot;
            }
        }
    };

    if kind == "multi" {
        let mut by_file: HashMap<String, u16> = HashMap::new();
        for (key, value) in defines {
            let Some(file) = value.as_str().filter(|f| !f.is_empty()) else { continue };
            let slot = match by_file.get(file) {
                Some(&s) => s,
                None => {
                    let sample = decode_file(&safe_join(dir, file)?)?;
                    let s = bank.add_slot(vec![sample]);
                    by_file.insert(file.to_owned(), s);
                    s
                }
            };
            if slot != NO_SLOT {
                assign(&mut bank, key, slot, &mut fallback);
            }
        }
    } else {
        let file = config
            .get("sound")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("config.json has no \"sound\" file"))?;
        let sprite = decode_file(&safe_join(dir, file)?)?;
        let mut by_range: HashMap<(u64, u64), u16> = HashMap::new();
        for (key, value) in defines {
            let Some(arr) = value.as_array() else { continue };
            let (Some(start), Some(len)) =
                (arr.first().and_then(|v| v.as_f64()), arr.get(1).and_then(|v| v.as_f64()))
            else {
                continue;
            };
            if len <= 0.0 {
                continue;
            }
            let k = (start.to_bits(), len.to_bits());
            let slot = match by_range.get(&k) {
                Some(&s) => s,
                None => {
                    let s = bank.add_slot(vec![slice_ms(&sprite, start, len)]);
                    by_range.insert(k, s);
                    s
                }
            };
            if slot != NO_SLOT {
                assign(&mut bank, key, slot, &mut fallback);
            }
        }
    }

    if fallback == NO_SLOT {
        bail!("no usable key sounds found in config.json");
    }
    for code in 0..NUM_CODES {
        if bank.down[code] == NO_SLOT {
            bank.down[code] = fallback;
        }
    }
    Ok(bank)
}

pub fn load_folder(dir: &Path, id: &str) -> Result<Bank> {
    let mut files: HashMap<(KeyGroup, bool), Vec<(u32, PathBuf)>> = HashMap::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "wav" | "ogg" | "mp3" | "flac") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        let mut parts = stem.split('_');
        let (Some(group), Some(stroke)) = (parts.next().and_then(KeyGroup::from_name), parts.next())
        else {
            continue;
        };
        let up = match stroke {
            "down" => false,
            "up" => true,
            _ => continue,
        };
        let n = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        files.entry((group, up)).or_default().push((n, path));
    }

    let mut bank = Bank::new(id);
    let mut slots: HashMap<(KeyGroup, bool), u16> = HashMap::new();
    for (key, mut list) in files {
        list.sort();
        let samples: Vec<Sample> = list.iter().map(|(_, p)| decode_file(p)).collect::<Result<_>>()?;
        slots.insert(key, bank.add_slot(samples));
    }
    let regular_down = *slots
        .get(&(KeyGroup::Regular, false))
        .ok_or_else(|| anyhow!("folder pack needs at least regular_down_1.wav"))?;
    let regular_up = slots.get(&(KeyGroup::Regular, true)).copied().unwrap_or(NO_SLOT);
    for group in KeyGroup::ALL {
        let down = slots.get(&(group, false)).copied().unwrap_or(regular_down);
        let up = slots.get(&(group, true)).copied().unwrap_or(regular_up);
        bank.map_group(group, down, up);
    }
    Ok(bank)
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '-' })
        .collect();
    let s = s.trim().trim_matches('-').to_string();
    if s.is_empty() { "pack".into() } else { s }
}

fn unique_dest(base: &str) -> PathBuf {
    let root = packs_dir();
    let base = sanitize(base);
    let mut dest = root.join(&base);
    let mut n = 2;
    while dest.exists() {
        dest = root.join(format!("{base} {n}"));
        n += 1;
    }
    dest
}

/// Finds the directory holding the manifest, at most two levels below `dir`.
fn find_pack_root(dir: &Path, depth: u32) -> Option<PathBuf> {
    if detect(dir).is_some() {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find_pack_root(&e.path(), depth - 1))
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Validates a freshly copied pack; removes it again if it can't be loaded.
fn finish_import(dest: PathBuf) -> Result<PackInfo> {
    let result = pack_info(&dest)
        .ok_or_else(|| anyhow!("no config.json or pack.json found"))
        .and_then(|info| load_bank(&info).map(|_| info));
    if result.is_err() {
        let _ = fs::remove_dir_all(&dest);
    }
    result
}

pub fn import_folder(src: &Path) -> Result<PackInfo> {
    let root = find_pack_root(src, 2)
        .ok_or_else(|| anyhow!("that folder has no Mechvibes config.json or Clatter pack.json"))?;
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let dest = unique_dest(&name);
    copy_dir(&root, &dest)?;
    finish_import(dest)
}

pub fn import_zip(zip_path: &Path) -> Result<PackInfo> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("not a valid .zip file")?;

    // Locate the shallowest manifest to know which folder inside the zip is the pack.
    let mut prefix: Option<PathBuf> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let Some(path) = entry.enclosed_name() else { continue };
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname == "config.json" || fname == "pack.json" {
            let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
            if prefix.as_ref().is_none_or(|p| parent.components().count() < p.components().count()) {
                prefix = Some(parent);
            }
        }
    }
    let prefix = prefix.ok_or_else(|| anyhow!("the .zip has no Mechvibes config.json or Clatter pack.json"))?;
    let name = prefix
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .or_else(|| zip_path.file_stem().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    let dest = unique_dest(&name);
    fs::create_dir_all(&dest)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(path) = entry.enclosed_name() else { continue };
        let Ok(rel) = path.strip_prefix(&prefix) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut f = fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    finish_import(dest)
}

pub fn remove_pack(id: &str) -> Result<()> {
    let info = find_pack(id).ok_or_else(|| anyhow!("pack not found"))?;
    fs::remove_dir_all(&info.dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample((s * 30000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn tone(n: usize, f: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * f / 44_100.0 * std::f32::consts::TAU).sin() * 0.5).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("clatter-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mechvibes_single() {
        let dir = temp_dir("mv-single");
        write_wav(&dir.join("sound.wav"), &tone(44_100, 440.0)); // 1 s
        fs::write(
            dir.join("config.json"),
            r#"{"id":"x","name":"Test Single","key_define_type":"single","sound":"sound.wav",
               "defines":{"30":[0,100],"57":[200,150],"28":[400,120],"3612":[400,120],"1":null}}"#,
        )
        .unwrap();
        let info = pack_info(&dir).unwrap();
        assert_eq!(info.name, "Test Single");
        assert_eq!(info.kind, PackKind::Mechvibes);
        let bank = load_bank(&info).unwrap();
        assert_eq!(bank.slots.len(), 3); // enter and keypad enter share one slice
        let a = bank.down[0x01E] as usize;
        let space = bank.down[0x039] as usize;
        assert_ne!(a, space);
        let len = bank.samples[bank.slots[space][0] as usize].data.len();
        assert!((len as i64 - 6615).abs() <= 1, "150 ms at 44.1 kHz, got {len}");
        // Undefined keys fall back to the "A" sound; no key-up sounds.
        assert_eq!(bank.down[0x010] as usize, a);
        assert!(bank.up.iter().all(|&s| s == NO_SLOT));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mechvibes_multi_with_up() {
        let dir = temp_dir("mv-multi");
        write_wav(&dir.join("a.wav"), &tone(2000, 300.0));
        write_wav(&dir.join("b.wav"), &tone(3000, 600.0));
        fs::write(
            dir.join("config.json"),
            r#"{"name":"Multi","key_define_type":"multi",
               "defines":{"30":"a.wav","31":"a.wav","57":"b.wav","57-up":"a.wav","2":null}}"#,
        )
        .unwrap();
        let bank = load_bank(&pack_info(&dir).unwrap()).unwrap();
        assert_eq!(bank.slots.len(), 2);
        assert_eq!(bank.down[0x01E], bank.down[0x01F]);
        assert_ne!(bank.up[0x039], NO_SLOT);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mechvibes_rejects_path_traversal() {
        let dir = temp_dir("mv-evil");
        fs::write(
            dir.join("config.json"),
            r#"{"key_define_type":"multi","defines":{"30":"../../secret.wav"}}"#,
        )
        .unwrap();
        assert!(load_bank(&pack_info(&dir).unwrap()).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_pack() {
        let dir = temp_dir("folder");
        fs::write(dir.join("pack.json"), r#"{"name":"Mine"}"#).unwrap();
        write_wav(&dir.join("regular_down_1.wav"), &tone(2000, 300.0));
        write_wav(&dir.join("regular_down_2.wav"), &tone(2000, 320.0));
        write_wav(&dir.join("regular_up_1.wav"), &tone(1000, 500.0));
        write_wav(&dir.join("space_down_1.wav"), &tone(4000, 150.0));
        write_wav(&dir.join("notes.wav"), &tone(100, 100.0)); // ignored: bad name
        let bank = load_bank(&pack_info(&dir).unwrap()).unwrap();
        assert_eq!(bank.slots.len(), 3);
        let reg = bank.down[0x01E] as usize;
        assert_eq!(bank.slots[reg].len(), 2);
        assert_ne!(bank.down[0x039], bank.down[0x01E]);
        assert_eq!(bank.down[0x01C], bank.down[0x01E]); // enter falls back to regular
        assert_eq!(bank.up[0x039], bank.up[0x01E]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
