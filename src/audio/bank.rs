//! A loaded set of switch sounds, ready for the mixer.

use crate::keymap::{self, KeyGroup, NUM_CODES};

pub const NO_SLOT: u16 = u16::MAX;

/// Mono PCM sample at its own sample rate (the mixer resamples on the fly).
pub struct Sample {
    pub data: Vec<f32>,
    pub rate: f32,
}

pub struct Bank {
    pub id: String,
    pub samples: Vec<Sample>,
    /// Each slot is a list of interchangeable variants (sample indices).
    pub slots: Vec<Vec<u32>>,
    pub down: Box<[u16; NUM_CODES]>,
    pub up: Box<[u16; NUM_CODES]>,
}

impl Bank {
    pub fn new(id: impl Into<String>) -> Self {
        Bank {
            id: id.into(),
            samples: Vec::new(),
            slots: Vec::new(),
            down: Box::new([NO_SLOT; NUM_CODES]),
            up: Box::new([NO_SLOT; NUM_CODES]),
        }
    }

    /// Adds a slot made of the given variants and returns its index, or `NO_SLOT` if empty.
    pub fn add_slot(&mut self, variants: Vec<Sample>) -> u16 {
        let variants: Vec<Sample> = variants.into_iter().filter(|s| !s.data.is_empty()).collect();
        if variants.is_empty() {
            return NO_SLOT;
        }
        let mut ids = Vec::with_capacity(variants.len());
        for s in variants {
            ids.push(self.samples.len() as u32);
            self.samples.push(s);
        }
        self.slots.push(ids);
        (self.slots.len() - 1) as u16
    }

    /// Points every key of `group` at the given down/up slots.
    pub fn map_group(&mut self, group: KeyGroup, down: u16, up: u16) {
        for code in 0..NUM_CODES as u16 {
            if keymap::group(code) == group {
                self.down[code as usize] = down;
                self.up[code as usize] = up;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}
