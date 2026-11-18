//! Low-latency audio output.
//!
//! * The keyboard hook pushes [`KeyEvent`]s into a lock-free ring.
//! * The cpal callback (the [`Mixer`]) pops them, picks a sound variant,
//!   randomises pitch/gain, pans it by key position and mixes all voices.
//!   It never allocates or blocks.
//! * A control thread owns the cpal stream: it (re)opens the output device,
//!   and pauses the stream after a while without typing so Windows can still
//!   sleep (an active audio stream blocks automatic sleep).

use std::f32::consts::{FRAC_PI_4, SQRT_2};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};

use super::bank::{Bank, NO_SLOT};
use crate::keymap;

const MAX_VOICES: usize = 48;
/// Key events older than this when they reach the mixer are dropped.
const MAX_EVENT_AGE_MS: u32 = 300;
/// Pause the output stream after this long without sounds.
const IDLE_PAUSE_MS: u32 = 15_000;
const TARGET_BUFFER_FRAMES: u32 = 256;

static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the app started (wraps after ~49 days; compared with wrapping_sub).
pub fn now_ms() -> u32 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub code: u16,
    pub down: bool,
    pub t_ms: u32,
}

enum Cmd {
    Bank(Arc<Bank>),
    Play(KeyEvent),
}

#[derive(Default)]
struct AtomicF32(AtomicU32);

impl AtomicF32 {
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// Live parameters read by the audio callback.
#[derive(Default)]
pub struct Params {
    gain: AtomicF32,
    pitch_semitones: AtomicF32,
    spatial: AtomicF32,
}

impl Params {
    /// `volume` is the 0..1 slider value; mapped to a perceptual gain curve.
    pub fn set_volume(&self, volume: f32) {
        self.gain.set(volume.clamp(0.0, 1.0).powf(1.7));
    }
    pub fn set_pitch_range(&self, semitones: f32) {
        self.pitch_semitones.set(semitones.clamp(0.0, 4.0));
    }
    pub fn set_spatial(&self, width: f32) {
        self.spatial.set(width.clamp(0.0, 1.0));
    }
}

struct Voice {
    sample: u32,
    pos: usize,
    frac: f32,
    step: f32,
    gl: f32,
    gr: f32,
}

struct Mixer {
    keys: rtrb::Consumer<KeyEvent>,
    cmds: rtrb::Consumer<Cmd>,
    bank: Option<Arc<Bank>>,
    voices: Vec<Voice>,
    params: Arc<Params>,
    rng: fastrand::Rng,
    last_sample: u32,
    out_rate: f32,
    scratch: Vec<f32>,
    activity: Arc<AtomicU32>,
}

impl Mixer {
    fn process_events(&mut self) {
        while let Ok(cmd) = self.cmds.pop() {
            match cmd {
                Cmd::Bank(bank) => {
                    // Voices index into the old bank's samples.
                    self.voices.clear();
                    // The old Arc is only decremented here; the main thread
                    // holds another reference and frees it off the audio thread.
                    self.bank = Some(bank);
                }
                Cmd::Play(ev) => self.trigger(ev),
            }
        }
        let now = now_ms();
        while let Ok(ev) = self.keys.pop() {
            if now.wrapping_sub(ev.t_ms) <= MAX_EVENT_AGE_MS {
                self.trigger(ev);
            }
        }
    }

    fn trigger(&mut self, ev: KeyEvent) {
        let Some(bank) = &self.bank else { return };
        let code = (ev.code as usize).min(keymap::NUM_CODES - 1);
        let slot = if ev.down { bank.down[code] } else { bank.up[code] };
        if slot == NO_SLOT {
            return;
        }
        let variants = &bank.slots[slot as usize];
        if variants.is_empty() {
            return;
        }
        let mut i = self.rng.usize(..variants.len());
        if variants.len() > 1 && variants[i] == self.last_sample {
            i = (i + 1) % variants.len();
        }
        let sample_id = variants[i];
        let sample = &bank.samples[sample_id as usize];

        let semis = self.params.pitch_semitones.get();
        let ratio = 2f32.powf((self.rng.f32() * 2.0 - 1.0) * semis / 12.0);
        let gain = 10f32.powf((self.rng.f32() * 2.0 - 1.0) * 1.5 / 20.0);
        let pan = keymap::pan(ev.code) * self.params.spatial.get();
        let angle = (pan + 1.0) * FRAC_PI_4;

        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            sample: sample_id,
            pos: 0,
            frac: 0.0,
            step: sample.rate / self.out_rate * ratio,
            gl: angle.cos() * SQRT_2 * 0.85 * gain,
            gr: angle.sin() * SQRT_2 * 0.85 * gain,
        });
        self.last_sample = sample_id;
        self.activity.store(now_ms(), Ordering::Relaxed);
    }

    fn render<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize) {
        let frames = out.len() / channels.max(1);
        if self.scratch.len() < frames * 2 {
            // Only happens if the device hands us a bigger buffer than ever before.
            self.scratch.resize(frames * 2, 0.0);
        }
        self.process_events();

        let buf = &mut self.scratch[..frames * 2];
        buf.fill(0.0);
        if let Some(bank) = &self.bank {
            for v in self.voices.iter_mut() {
                let data = &bank.samples[v.sample as usize].data;
                for f in 0..frames {
                    if v.pos + 1 >= data.len() {
                        v.pos = usize::MAX;
                        break;
                    }
                    let s = data[v.pos] + (data[v.pos + 1] - data[v.pos]) * v.frac;
                    buf[2 * f] += s * v.gl;
                    buf[2 * f + 1] += s * v.gr;
                    v.frac += v.step;
                    let adv = v.frac as usize;
                    v.pos += adv;
                    v.frac -= adv as f32;
                }
            }
            self.voices.retain(|v| v.pos != usize::MAX);
        }

        let gain = self.params.gain.get();
        for (frame, lr) in out.chunks_mut(channels).zip(buf.chunks(2)) {
            let l = limit(lr[0] * gain);
            let r = limit(lr[1] * gain);
            match frame.len() {
                1 => frame[0] = T::from_sample((l + r) * 0.5),
                _ => {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for x in &mut frame[2..] {
                        *x = T::from_sample(0.0);
                    }
                }
            }
        }
    }
}

#[inline]
fn limit(x: f32) -> f32 {
    const T: f32 = 0.85;
    let a = x.abs();
    if a <= T { x } else { x.signum() * (T + (1.0 - T) * ((a - T) / (1.0 - T)).tanh()) }
}

struct Control {
    mixer: Mutex<Mixer>,
    /// Requested output device name; `None` follows the Windows default.
    device: Mutex<Option<String>>,
    rebuild: AtomicBool,
    quit: AtomicBool,
    paused: AtomicBool,
    activity: Arc<AtomicU32>,
    thread: OnceLock<Thread>,
}

/// Cheap handle the keyboard hook uses to wake a paused stream.
#[derive(Clone)]
pub struct Waker(Arc<Control>);

impl Waker {
    #[inline]
    pub fn wake(&self) {
        self.0.activity.store(now_ms(), Ordering::Relaxed);
        if self.0.paused.load(Ordering::Relaxed)
            && let Some(t) = self.0.thread.get()
        {
            t.unpark();
        }
    }
}

pub struct Audio {
    params: Arc<Params>,
    cmd_tx: rtrb::Producer<Cmd>,
    keepalive: Vec<Arc<Bank>>,
    current: Option<Arc<Bank>>,
    control: Arc<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Audio {
    /// Starts the audio control thread. Returns the engine and the producer
    /// end of the key-event ring for the keyboard hook.
    pub fn start(device: Option<String>) -> (Audio, rtrb::Producer<KeyEvent>, Waker) {
        now_ms();
        let (key_tx, key_rx) = rtrb::RingBuffer::new(256);
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(64);
        let params = Arc::new(Params::default());
        params.set_volume(0.6);
        let activity = Arc::new(AtomicU32::new(now_ms()));
        let mixer = Mixer {
            keys: key_rx,
            cmds: cmd_rx,
            bank: None,
            voices: Vec::with_capacity(MAX_VOICES + 1),
            params: params.clone(),
            rng: fastrand::Rng::new(),
            last_sample: u32::MAX,
            out_rate: 48_000.0,
            scratch: vec![0.0; 8192 * 2],
            activity: activity.clone(),
        };
        let control = Arc::new(Control {
            mixer: Mutex::new(mixer),
            device: Mutex::new(device),
            rebuild: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            paused: AtomicBool::new(true),
            activity,
            thread: OnceLock::new(),
        });
        let c = control.clone();
        let thread = thread::Builder::new()
            .name("clatter-audio".into())
            .spawn(move || control_loop(c))
            .expect("spawn audio thread");
        let _ = control.thread.set(thread.thread().clone());
        let waker = Waker(control.clone());
        (
            Audio { params, cmd_tx, keepalive: Vec::new(), current: None, control, thread: Some(thread) },
            key_tx,
            waker,
        )
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    pub fn set_bank(&mut self, bank: Bank) {
        let bank = Arc::new(bank);
        // Free banks the mixer no longer references (on this thread, not the audio thread).
        self.keepalive.retain(|b| Arc::strong_count(b) > 1);
        self.keepalive.push(bank.clone());
        self.current = Some(bank.clone());
        if self.cmd_tx.push(Cmd::Bank(bank)).is_err() {
            log::warn!("audio command queue full; switch change dropped");
        }
        self.wake();
    }

    pub fn current_bank_id(&self) -> Option<&str> {
        self.current.as_deref().map(|b| b.id.as_str())
    }

    /// Plays a key sound regardless of the enabled/excluded state (for previews).
    pub fn play(&mut self, code: u16, down: bool) {
        let _ = self.cmd_tx.push(Cmd::Play(KeyEvent { code, down, t_ms: now_ms() }));
        self.wake();
    }

    fn wake(&self) {
        Waker(self.control.clone()).wake();
    }

    pub fn set_device(&mut self, name: Option<String>) {
        let mut d = self.control.device.lock().unwrap();
        if *d != name {
            *d = name;
            self.control.rebuild.store(true, Ordering::Relaxed);
            if let Some(t) = self.control.thread.get() {
                t.unpark();
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.control.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Names of the available output devices.
pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|x| x.name().to_owned())).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

fn device_key(d: &cpal::Device) -> String {
    d.id().map(|id| id.to_string()).unwrap_or_default()
}

fn find_device(host: &cpal::Host, name: &Option<String>) -> Option<cpal::Device> {
    if let Some(name) = name
        && let Ok(mut devices) = host.output_devices()
        && let Some(d) = devices.find(|d| d.description().is_ok_and(|x| x.name() == name))
    {
        return Some(d);
    }
    host.default_output_device()
}

fn build_stream(control: &Arc<Control>, device: &cpal::Device) -> anyhow::Result<cpal::Stream> {
    let supported = device.default_output_config()?;
    let mut config: StreamConfig = supported.config();
    let mut fixed = None;
    if let SupportedBufferSize::Range { min, max } = *supported.buffer_size() {
        fixed = Some(TARGET_BUFFER_FRAMES.clamp(min, max));
    }
    control.mixer.lock().unwrap().out_rate = config.sample_rate as f32;

    let format = supported.sample_format();
    let attempt = |config: StreamConfig| -> anyhow::Result<cpal::Stream> {
        Ok(match format {
            SampleFormat::F32 => typed::<f32>(control, device, config)?,
            SampleFormat::I16 => typed::<i16>(control, device, config)?,
            SampleFormat::U16 => typed::<u16>(control, device, config)?,
            SampleFormat::I32 => typed::<i32>(control, device, config)?,
            SampleFormat::F64 => typed::<f64>(control, device, config)?,
            SampleFormat::U8 => typed::<u8>(control, device, config)?,
            other => anyhow::bail!("unsupported sample format {other}"),
        })
    };
    if let Some(frames) = fixed {
        config.buffer_size = BufferSize::Fixed(frames);
        match attempt(config) {
            Ok(s) => return Ok(s),
            Err(e) => log::info!("fixed buffer of {frames} frames refused ({e}); using default"),
        }
    }
    config.buffer_size = BufferSize::Default;
    attempt(config)
}

fn typed<T: SizedSample + FromSample<f32>>(
    control: &Arc<Control>,
    device: &cpal::Device,
    config: StreamConfig,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let c = control.clone();
    let e = control.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _| match c.mixer.try_lock() {
            Ok(mut m) => m.render(data, channels),
            Err(_) => data.iter_mut().for_each(|x| *x = T::from_sample(0.0f32)),
        },
        move |err: cpal::Error| {
            use cpal::ErrorKind::*;
            match err.kind() {
                Xrun | RealtimeDenied => {}
                _ => {
                    log::warn!("audio stream error: {err}");
                    e.rebuild.store(true, Ordering::Relaxed);
                    if let Some(t) = e.thread.get() {
                        t.unpark();
                    }
                }
            }
        },
        None,
    )
}

fn control_loop(control: Arc<Control>) {
    let host = cpal::default_host();
    let mut stream: Option<cpal::Stream> = None;
    let mut stream_device = String::new();
    let mut playing = false;
    let mut next_retry = Instant::now();

    while !control.quit.load(Ordering::Relaxed) {
        let idle = now_ms().wrapping_sub(control.activity.load(Ordering::Relaxed)) > IDLE_PAUSE_MS;
        let requested = control.device.lock().unwrap().clone();

        let mut rebuild = control.rebuild.swap(false, Ordering::Relaxed);
        // Waking up after a pause: follow the Windows default device if it changed.
        if stream.is_some() && !playing && !idle && requested.is_none() {
            let default = host.default_output_device().map(|d| device_key(&d)).unwrap_or_default();
            if default != stream_device {
                rebuild = true;
            }
        }
        if rebuild {
            stream = None;
            playing = false;
        }
        if stream.is_none() && (rebuild || Instant::now() >= next_retry) {
            match find_device(&host, &requested) {
                Some(device) => match build_stream(&control, &device) {
                    Ok(s) => {
                        stream_device = device_key(&device);
                        log::info!(
                            "audio output: {}",
                            device.description().map(|d| d.name().to_owned()).unwrap_or_default()
                        );
                        let _ = s.pause();
                        stream = Some(s);
                    }
                    Err(e) => {
                        log::warn!("could not open audio output: {e:#}");
                        next_retry = Instant::now() + Duration::from_secs(3);
                    }
                },
                None => next_retry = Instant::now() + Duration::from_secs(3),
            }
        }

        if let Some(s) = &stream {
            if idle && playing {
                let _ = s.pause();
                playing = false;
            } else if !idle && !playing {
                match s.play() {
                    Ok(()) => playing = true,
                    Err(e) => {
                        log::warn!("could not start audio stream: {e}");
                        stream = None;
                        next_retry = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
        }
        control.paused.store(!playing, Ordering::Relaxed);

        thread::park_timeout(if playing { Duration::from_millis(500) } else { Duration::from_secs(3) });
    }
}
