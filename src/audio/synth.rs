//! Procedurally synthesized switch sounds.
//!
//! Each keystroke is modelled as a short excitation (the stem bottoming out)
//! driving a handful of damped resonances (keycap, plate and case), plus a
//! burst of band-passed contact noise. Clicky switches add a click-jacket
//! transient, big keys add stabilizer rattle, and the typewriter rings a bell
//! on Enter. Everything is deterministic: the same profile always produces the
//! same set of sounds.

use std::f32::consts::{PI, TAU};

use super::bank::{Bank, Sample};
use crate::keymap::KeyGroup;

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const VARIANTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    Down,
    Up,
}

pub struct Profile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// (frequency Hz, decay ms, gain) of the body resonances.
    modes: &'static [(f32, f32, f32)],
    /// Width of the bottom-out impulse. Wider = softer, duller hit.
    impulse_ms: f32,
    noise_gain: f32,
    noise_decay_ms: f32,
    noise_center: f32,
    noise_q: f32,
    /// Click-jacket level (0 = linear/tactile switch).
    click: f32,
    click_freq: f32,
    /// How long before bottom-out the click happens on the downstroke.
    click_lead_ms: f32,
    up_gain: f32,
    up_shift: f32,
    lowpass: f32,
    /// Frequency multiplier for the space bar.
    big_shift: f32,
    rattle: f32,
    bell: f32,
    reflection: f32,
}

pub static PROFILES: &[Profile] = &[
    Profile {
        id: "obsidian",
        name: "Obsidian",
        description: "Deep, lubed linear. Low thock, foam-dampened case.",
        modes: &[
            (165.0, 30.0, 1.0),
            (330.0, 22.0, 0.75),
            (560.0, 14.0, 0.5),
            (920.0, 9.0, 0.35),
            (1500.0, 5.0, 0.2),
            (2400.0, 3.0, 0.1),
        ],
        impulse_ms: 1.1,
        noise_gain: 0.18,
        noise_decay_ms: 3.5,
        noise_center: 1100.0,
        noise_q: 0.9,
        click: 0.0,
        click_freq: 0.0,
        click_lead_ms: 0.0,
        up_gain: 0.32,
        up_shift: 1.55,
        lowpass: 5200.0,
        big_shift: 0.6,
        rattle: 0.25,
        bell: 0.0,
        reflection: 0.18,
    },
    Profile {
        id: "marshmallow",
        name: "Marshmallow",
        description: "Soft and creamy. Muted, rounded and quiet.",
        modes: &[
            (240.0, 16.0, 1.0),
            (470.0, 11.0, 0.6),
            (820.0, 7.0, 0.4),
            (1350.0, 5.0, 0.22),
        ],
        impulse_ms: 1.6,
        noise_gain: 0.12,
        noise_decay_ms: 3.0,
        noise_center: 900.0,
        noise_q: 0.7,
        click: 0.0,
        click_freq: 0.0,
        click_lead_ms: 0.0,
        up_gain: 0.4,
        up_shift: 1.35,
        lowpass: 3600.0,
        big_shift: 0.65,
        rattle: 0.12,
        bell: 0.0,
        reflection: 0.12,
    },
    Profile {
        id: "glass",
        name: "Glass",
        description: "Bright, crisp clack. Thin caps on a metal case.",
        modes: &[
            (480.0, 14.0, 0.55),
            (1150.0, 11.0, 1.0),
            (2050.0, 9.0, 0.75),
            (3300.0, 6.0, 0.5),
            (4700.0, 4.0, 0.3),
        ],
        impulse_ms: 0.35,
        noise_gain: 0.4,
        noise_decay_ms: 2.5,
        noise_center: 3200.0,
        noise_q: 1.1,
        click: 0.0,
        click_freq: 0.0,
        click_lead_ms: 0.0,
        up_gain: 0.55,
        up_shift: 1.2,
        lowpass: 13000.0,
        big_shift: 0.6,
        rattle: 0.4,
        bell: 0.0,
        reflection: 0.22,
    },
    Profile {
        id: "click-jacket",
        name: "Click Jacket",
        description: "Loud and clicky. Sharp click on the way down and up.",
        modes: &[
            (310.0, 18.0, 0.8),
            (720.0, 12.0, 0.7),
            (1450.0, 8.0, 0.5),
            (2600.0, 5.0, 0.3),
        ],
        impulse_ms: 0.7,
        noise_gain: 0.25,
        noise_decay_ms: 2.5,
        noise_center: 2000.0,
        noise_q: 1.0,
        click: 1.1,
        click_freq: 4300.0,
        click_lead_ms: 11.0,
        up_gain: 0.5,
        up_shift: 1.3,
        lowpass: 11000.0,
        big_shift: 0.6,
        rattle: 0.3,
        bell: 0.0,
        reflection: 0.15,
    },
    Profile {
        id: "typewriter",
        name: "Typewriter",
        description: "Metal type bars and a bell on Return.",
        modes: &[
            (210.0, 45.0, 0.7),
            (780.0, 35.0, 0.6),
            (1650.0, 28.0, 0.8),
            (2900.0, 22.0, 0.55),
            (4400.0, 16.0, 0.35),
        ],
        impulse_ms: 0.5,
        noise_gain: 0.45,
        noise_decay_ms: 4.0,
        noise_center: 2500.0,
        noise_q: 0.8,
        click: 0.0,
        click_freq: 0.0,
        click_lead_ms: 0.0,
        up_gain: 0.28,
        up_shift: 1.15,
        lowpass: 10000.0,
        big_shift: 0.55,
        rattle: 0.6,
        bell: 0.55,
        reflection: 0.3,
    },
    Profile {
        id: "chiclet",
        name: "Chiclet",
        description: "Low-profile laptop keys. Short, light and tidy.",
        modes: &[
            (700.0, 6.0, 0.6),
            (1500.0, 5.0, 1.0),
            (2800.0, 4.0, 0.6),
            (4200.0, 3.0, 0.3),
        ],
        impulse_ms: 0.5,
        noise_gain: 0.35,
        noise_decay_ms: 1.8,
        noise_center: 3500.0,
        noise_q: 1.2,
        click: 0.0,
        click_freq: 0.0,
        click_lead_ms: 0.0,
        up_gain: 0.3,
        up_shift: 1.15,
        lowpass: 9000.0,
        big_shift: 0.7,
        rattle: 0.2,
        bell: 0.0,
        reflection: 0.1,
    },
];

pub fn profile(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.id == id)
}

pub const DEFAULT_PROFILE: &str = "obsidian";

/// All sounds of a profile, grouped as (group, stroke) -> variants.
pub struct ProfileSounds {
    pub sets: Vec<(KeyGroup, Stroke, Vec<Vec<f32>>)>,
}

pub fn render_profile(p: &Profile) -> ProfileSounds {
    let mut sets = Vec::new();
    for group in KeyGroup::ALL {
        for stroke in [Stroke::Down, Stroke::Up] {
            let variants: Vec<Vec<f32>> = (0..VARIANTS)
                .map(|v| {
                    let mut rng = fastrand::Rng::with_seed(seed(p.id, group, stroke, v));
                    hit(p, group, stroke, &mut rng)
                })
                .collect();
            sets.push((group, stroke, variants));
        }
    }

    // Normalise the whole profile so a regular keystroke peaks around -3 dBFS,
    // keeping the relative level of up-strokes and big keys.
    let reference = sets
        .iter()
        .filter(|(g, s, _)| *g == KeyGroup::Regular && *s == Stroke::Down)
        .flat_map(|(_, _, v)| v.iter())
        .map(|s| peak(s))
        .fold(0.0f32, f32::max)
        .max(1e-6);
    let gain = 0.7 / reference;
    for (_, _, variants) in &mut sets {
        for s in variants.iter_mut() {
            for x in s.iter_mut() {
                *x = soft_clip(*x * gain);
            }
        }
    }
    ProfileSounds { sets }
}

pub fn build_bank(p: &Profile) -> Bank {
    let sounds = render_profile(p);
    let mut bank = Bank::new(p.id);
    for group in KeyGroup::ALL {
        let slot = |stroke: Stroke, bank: &mut Bank| {
            let variants = sounds
                .sets
                .iter()
                .find(|(g, s, _)| *g == group && *s == stroke)
                .map(|(_, _, v)| v.clone())
                .unwrap_or_default();
            bank.add_slot(
                variants
                    .into_iter()
                    .map(|data| Sample { data, rate: SAMPLE_RATE })
                    .collect(),
            )
        };
        let down = slot(Stroke::Down, &mut bank);
        let up = slot(Stroke::Up, &mut bank);
        bank.map_group(group, down, up);
    }
    bank
}

fn seed(id: &str, group: KeyGroup, stroke: Stroke, variant: usize) -> u64 {
    // FNV-1a over the identifying parts.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    id.bytes().for_each(&mut eat);
    group.name().bytes().for_each(&mut eat);
    eat(stroke as u8);
    eat(variant as u8);
    h
}

struct GroupShape {
    freq: f32,
    decay: f32,
    gain: f32,
    rattle: f32,
}

fn group_shape(p: &Profile, group: KeyGroup) -> GroupShape {
    match group {
        KeyGroup::Regular => GroupShape { freq: 1.0, decay: 1.0, gain: 1.0, rattle: 0.0 },
        KeyGroup::Wide => GroupShape { freq: 0.9, decay: 1.1, gain: 1.0, rattle: 0.15 },
        KeyGroup::Backspace => GroupShape { freq: 0.85, decay: 1.15, gain: 1.05, rattle: 0.3 },
        KeyGroup::Enter => GroupShape { freq: 0.82, decay: 1.2, gain: 1.1, rattle: 0.35 },
        KeyGroup::Space => GroupShape { freq: p.big_shift, decay: 1.45, gain: 1.2, rattle: 1.0 },
    }
}

fn jitter(rng: &mut fastrand::Rng, amount: f32) -> f32 {
    1.0 + (rng.f32() * 2.0 - 1.0) * amount
}

fn ms(x: f32) -> usize {
    (x * 0.001 * SAMPLE_RATE).round() as usize
}

/// Renders one keystroke.
pub fn hit(p: &Profile, group: KeyGroup, stroke: Stroke, rng: &mut fastrand::Rng) -> Vec<f32> {
    let shape = group_shape(p, group);
    let up = stroke == Stroke::Up;

    let freq_mul = shape.freq * if up { p.up_shift } else { 1.0 };
    let decay_mul = shape.decay * if up { 0.55 } else { 1.0 };
    let level = shape.gain * if up { p.up_gain } else { 1.0 };
    let impulse_ms = p.impulse_ms * if up { 0.7 } else { 1.0 };

    // Clicky switches click first, then bottom out.
    let click_level = if p.click > 0.0 { p.click * if up { 0.6 } else { 1.0 } } else { 0.0 };
    let body_offset = if click_level > 0.0 && !up { ms(p.click_lead_ms * jitter(rng, 0.15)) } else { 0 };

    let longest_decay = p
        .modes
        .iter()
        .map(|m| m.1)
        .fold(0.0f32, f32::max)
        * decay_mul;
    let mut len_ms = (longest_decay * 7.0).min(350.0) + 15.0;
    let ring_bell = p.bell > 0.0 && group == KeyGroup::Enter && !up;
    if ring_bell {
        len_ms = len_ms.max(900.0);
    }
    let len = ms(len_ms) + body_offset;
    let mut out = vec![0.0f32; len];

    // --- Excitation: Hann pulse plus a short noise burst for texture.
    let mut excite = vec![0.0f32; len];
    let pulse_len = ms(impulse_ms).max(2);
    for i in 0..pulse_len {
        let w = 0.5 - 0.5 * (TAU * i as f32 / pulse_len as f32).cos();
        excite[body_offset + i] += w;
    }
    let burst_tau = p.noise_decay_ms * 0.001 * SAMPLE_RATE;
    for i in 0..ms(p.noise_decay_ms * 6.0).min(len - body_offset) {
        excite[body_offset + i] += (rng.f32() * 2.0 - 1.0) * 0.35 * (-(i as f32) / burst_tau).exp();
    }

    // --- Body resonances.
    let mut tmp = vec![0.0f32; len];
    for &(f, decay_ms, gain) in p.modes {
        let f = f * freq_mul * jitter(rng, 0.035);
        if f >= SAMPLE_RATE * 0.45 {
            continue;
        }
        let decay = decay_ms * decay_mul * jitter(rng, 0.12);
        let gain = gain * jitter(rng, 0.15);
        resonate(&excite, &mut tmp, f, decay);
        let pk = peak(&tmp).max(1e-9);
        for (o, t) in out.iter_mut().zip(&tmp) {
            *o += t * gain / pk;
        }
    }

    // --- Contact noise (direct, band-passed).
    {
        let center = (p.noise_center * freq_mul.sqrt()).min(SAMPLE_RATE * 0.4);
        let mut bp = Biquad::bandpass(center, p.noise_q);
        let tau = p.noise_decay_ms * 0.001 * SAMPLE_RATE * if up { 0.8 } else { 1.0 };
        let attack = ms(0.2).max(1) as f32;
        let n = ms(p.noise_decay_ms * 8.0).min(len - body_offset);
        let g = p.noise_gain * if up { 0.8 } else { 1.0 } * 2.5;
        for i in 0..n {
            let env = (i as f32 / attack).min(1.0) * (-(i as f32) / tau).exp();
            out[body_offset + i] += bp.process(rng.f32() * 2.0 - 1.0) * env * g;
        }
    }

    // --- Click jacket.
    if click_level > 0.0 {
        let f = p.click_freq * jitter(rng, 0.05);
        let mut bp = Biquad::bandpass(f, 3.0);
        let tau = 1.2 * 0.001 * SAMPLE_RATE;
        let n = ms(8.0).min(len);
        let mut click = vec![0.0f32; len];
        for (i, c) in click.iter_mut().enumerate().take(n) {
            let env = (-(i as f32) / tau).exp();
            *c = bp.process(rng.f32() * 2.0 - 1.0) * env;
        }
        // A short ring at the click frequency gives the "tick" its pitch.
        let mut ring = vec![0.0f32; len];
        let mut imp = vec![0.0f32; len];
        imp[0] = 1.0;
        resonate(&imp, &mut ring, f * 0.92, 3.0);
        let (pc, pr) = (peak(&click).max(1e-9), peak(&ring).max(1e-9));
        for i in 0..len {
            out[i] += (click[i] / pc * 0.8 + ring[i] / pr * 0.5) * click_level;
        }
    }

    // --- Stabilizer rattle on big keys.
    let rattle = shape.rattle * p.rattle;
    if rattle > 0.0 {
        let delay = ms(5.0 + rng.f32() * 6.0) + body_offset;
        if delay < len {
            let mut imp = vec![0.0f32; len];
            imp[delay] = 1.0;
            imp[(delay + ms(0.4)).min(len - 1)] = -0.6;
            for &(f, d) in &[(2300.0f32, 6.0f32), (3700.0, 4.5), (5200.0, 3.5)] {
                resonate(&imp, &mut tmp, f * jitter(rng, 0.06), d * jitter(rng, 0.2));
                let pk = peak(&tmp).max(1e-9);
                for (o, t) in out.iter_mut().zip(&tmp) {
                    *o += t / pk * rattle * 0.35;
                }
            }
        }
    }

    // --- Typewriter bell on Return.
    if ring_bell {
        let start = ms(30.0) + body_offset;
        let f0 = 2650.0 * jitter(rng, 0.01);
        for (k, (mul, amp, decay_ms)) in [(1.0f32, 1.0f32, 400.0f32), (2.76, 0.3, 150.0), (5.4, 0.1, 60.0)]
            .into_iter()
            .enumerate()
        {
            let f = f0 * mul;
            let tau = decay_ms * 0.001 * SAMPLE_RATE;
            let phase = k as f32 * 0.7;
            for (n, o) in out[start..].iter_mut().enumerate() {
                let t = n as f32;
                *o += (TAU * f * t / SAMPLE_RATE + phase).sin() * (-t / tau).exp() * amp * p.bell;
            }
        }
    }

    // --- Tone shaping: DC-blocking high-pass and the profile's low-pass.
    let mut hp = Biquad::highpass(70.0, 0.707);
    let mut lp = Biquad::lowpass(p.lowpass.min(SAMPLE_RATE * 0.45), 0.707);
    for x in out.iter_mut() {
        *x = lp.process(hp.process(*x));
    }

    // --- Desk / case reflections.
    if p.reflection > 0.0 {
        let taps = [(ms(4.5), p.reflection), (ms(9.7), p.reflection * 0.5)];
        let dry = out.clone();
        for (d, g) in taps {
            for i in d..len {
                out[i] += dry[i - d] * g;
            }
        }
    }

    for x in out.iter_mut() {
        *x *= level;
    }

    // Fade the tail and trim silence.
    let fade = ms(10.0).min(len);
    for i in 0..fade {
        out[len - 1 - i] *= i as f32 / fade as f32;
    }
    let pk = peak(&out);
    if let Some(last) = out.iter().rposition(|x| x.abs() > pk * 0.001) {
        out.truncate((last + 1).max(ms(5.0)).min(len));
    }
    out
}

/// Two-pole resonator. `decay_ms` is the amplitude time constant.
fn resonate(input: &[f32], output: &mut [f32], freq: f32, decay_ms: f32) {
    let r = (-1.0 / (decay_ms.max(0.1) * 0.001 * SAMPLE_RATE)).exp();
    let theta = TAU * freq / SAMPLE_RATE;
    let a1 = 2.0 * r * theta.cos();
    let a2 = -r * r;
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    for (o, &x) in output.iter_mut().zip(input) {
        let y = x * (1.0 - r) + a1 * y1 + a2 * y2;
        y2 = y1;
        y1 = y;
        *o = y;
    }
}

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn from(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    fn coeffs(freq: f32, q: f32) -> (f32, f32) {
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        (w.cos(), w.sin() / (2.0 * q))
    }

    fn lowpass(freq: f32, q: f32) -> Self {
        let (c, alpha) = Self::coeffs(freq, q);
        Self::from((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn highpass(freq: f32, q: f32) -> Self {
        let (c, alpha) = Self::coeffs(freq, q);
        Self::from((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn bandpass(freq: f32, q: f32) -> Self {
        let (c, alpha) = Self::coeffs(freq, q);
        Self::from(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn process(&mut self, x: f32) -> f32 {
        // Transposed direct form II.
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

pub fn peak(s: &[f32]) -> f32 {
    s.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

/// Transparent below 0.8, smoothly limits above so nothing exceeds 1.0.
fn soft_clip(x: f32) -> f32 {
    const T: f32 = 0.8;
    let a = x.abs();
    if a <= T {
        x
    } else {
        x.signum() * (T + (1.0 - T) * ((a - T) / (1.0 - T)).tanh())
    }
}

/// Writes every sound of every profile as 16-bit WAV files, laid out as
/// folder packs (`<dir>/<profile>/<group>_<down|up>_<n>.wav` + `pack.json`).
pub fn export_wavs(dir: &std::path::Path) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    for p in PROFILES {
        let pdir = dir.join(p.id);
        std::fs::create_dir_all(&pdir)?;
        std::fs::write(
            pdir.join("pack.json"),
            serde_json::to_string_pretty(&serde_json::json!({ "name": format!("{} (exported)", p.name) }))?,
        )?;
        for (group, stroke, variants) in render_profile(p).sets {
            for (i, v) in variants.iter().enumerate() {
                let name = format!(
                    "{}_{}_{}.wav",
                    group.name(),
                    if stroke == Stroke::Down { "down" } else { "up" },
                    i + 1
                );
                let mut w = hound::WavWriter::create(pdir.join(name), spec)?;
                for &x in v {
                    w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16)?;
                }
                w.finalize()?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let p = profile("glass").unwrap();
        let a = render_profile(p);
        let b = render_profile(p);
        for ((_, _, va), (_, _, vb)) in a.sets.iter().zip(&b.sets) {
            assert_eq!(va, vb);
        }
    }

    #[test]
    fn levels_are_sane() {
        for p in PROFILES {
            for (group, stroke, variants) in render_profile(p).sets {
                assert_eq!(variants.len(), VARIANTS);
                for v in variants {
                    let pk = peak(&v);
                    assert!(pk <= 1.0, "{} {:?} {:?} peak {pk}", p.id, group, stroke);
                    assert!(pk > 0.02, "{} {:?} {:?} too quiet: {pk}", p.id, group, stroke);
                    assert!(v.iter().all(|x| x.is_finite()));
                    assert!(v.len() < SAMPLE_RATE as usize, "{} too long", p.id);
                }
            }
        }
    }

    #[test]
    fn regular_down_peaks_near_minus_3db() {
        for p in PROFILES {
            let sounds = render_profile(p);
            let pk = sounds
                .sets
                .iter()
                .filter(|(g, s, _)| *g == KeyGroup::Regular && *s == Stroke::Down)
                .flat_map(|(_, _, v)| v.iter())
                .map(|v| peak(v))
                .fold(0.0, f32::max);
            assert!((pk - 0.7).abs() < 0.05, "{} peak {pk}", p.id);
        }
    }

    #[test]
    fn bank_maps_every_key() {
        let bank = build_bank(profile(DEFAULT_PROFILE).unwrap());
        assert!(bank.down.iter().all(|&s| s != super::super::bank::NO_SLOT));
        assert!(bank.up.iter().all(|&s| s != super::super::bank::NO_SLOT));
        assert_eq!(bank.slots.len(), KeyGroup::ALL.len() * 2);
    }
}
