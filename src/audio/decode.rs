//! Decoding audio files (WAV, OGG Vorbis, MP3, FLAC) to mono f32.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::bank::Sample;

pub fn decode_file(path: &Path) -> Result<Sample> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    decode_bytes(bytes, ext).with_context(|| format!("decoding {}", path.display()))
}

pub fn decode_bytes(bytes: Vec<u8>, extension: &str) -> Result<Sample> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if !extension.is_empty() {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("no audio track"))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("not an audio track"))?
        .clone();
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default())?;

    let mut mono: Vec<f32> = Vec::new();
    let mut rate = params.sample_rate.unwrap_or(0);
    let mut interleaved: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec();
                rate = spec.rate();
                let channels = spec.channels().count().max(1);
                interleaved.clear();
                buf.copy_to_vec_interleaved(&mut interleaved);
                for frame in interleaved.chunks(channels) {
                    mono.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    if rate == 0 {
        return Err(anyhow!("unknown sample rate"));
    }
    Ok(Sample { data: mono, rate: rate as f32 })
}

/// Returns `[start, start+len)` of `sample` as a new sample (times in ms).
pub fn slice_ms(sample: &Sample, start_ms: f64, len_ms: f64) -> Sample {
    let sr = sample.rate as f64;
    let a = ((start_ms * sr / 1000.0) as usize).min(sample.data.len());
    let b = (((start_ms + len_ms) * sr / 1000.0) as usize).clamp(a, sample.data.len());
    let mut data = sample.data[a..b].to_vec();
    // Tiny fades so sprite cuts never click.
    let fade = (sr * 0.002) as usize;
    let n = data.len();
    for i in 0..fade.min(n / 2) {
        let g = i as f32 / fade as f32;
        data[i] *= g;
        data[n - 1 - i] *= g;
    }
    Sample { data, rate: sample.rate }
}
