// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mango Beat's parameters: the two sections' mixes, their slot choices and the envelope cycle's length. The
//! envelope grids travel in the plugin state's fields (`grid`), written by the editor and read by the sound
//! through the shared grids.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::live::LiveEdits;

use crate::dsp;
use crate::grid::SharedGrid;

pub const PARAMS: usize = 5;
pub const TIME_MIX: usize = 0;
pub const VOL_MIX: usize = 1;
pub const TIME_SLOT: usize = 2;
pub const VOL_SLOT: usize = 3;
pub const CYCLE: usize = 4;

/// The cycle choices' lengths in beats: half a bar, one, two and four bars of 4/4 (the host's transport carries
/// beats only, so a bar is four beats).
pub const CYCLE_BEATS: [f32; 4] = [2.0, 4.0, 8.0, 16.0];
/// The cycle choices' names, in `CYCLE_BEATS`' order.
pub const CYCLE_NAMES: [&str; 4] = ["½ bar", "1 bar", "2 bars", "4 bars"];

/// One setting: id and name.
#[derive(Clone, Debug)]
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
}

pub const DEFS: [Def; PARAMS] = [
    Def { id: "time_mix", name: "Time mix" },
    Def { id: "vol_mix", name: "Volume mix" },
    Def { id: "time_slot", name: "Time slot" },
    Def { id: "vol_slot", name: "Volume slot" },
    Def { id: "cycle", name: "Cycle" },
];

/// What a new Mango Beat starts on: both sections fully in, on slot A, the cycle over four bars.
pub const START: [i32; PARAMS] = [10000, 10000, 0, 0, 3];

/// The ranges (in `crates/os-app` the presets and the mixer read them too).
pub const RANGES: [(i32, i32); PARAMS] = [(0, 10000), (0, 10000), (0, 3), (0, 3), (0, 3)];

/// The state field the grids travel in.
pub const GRID_FIELD: &str = "grid";

pub fn index(id: &str) -> Option<usize> {
    DEFS.iter().position(|d| d.id == id)
}

/// A setting's value text: the mixes in percent, the slots by letter, the cycle in bars.
pub fn value_text(k: usize, v: i32) -> String {
    match k {
        TIME_MIX | VOL_MIX => format!("{:.1}%", f64::from(v) / 100.0),
        CYCLE => CYCLE_NAMES[(v.clamp(0, 3)) as usize].to_string(),
        _ => ["A", "B", "C", "D"][(v.clamp(0, 3)) as usize].to_string(),
    }
}

/// The number a value text shows (`None` for the slots).
pub fn number(k: usize, v: i32) -> Option<f64> {
    match k {
        TIME_MIX | VOL_MIX => Some(f64::from(v) / 100.0),
        _ => None,
    }
}

pub struct BeatParams {
    /// The settings in `DEFS`' order.
    pub p: Vec<IntParam>,
    /// The envelope grids (the editor writes, the sound reads, the state serializes).
    pub grid: Arc<SharedGrid>,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
    /// Bumped whenever the grids move from outside (the editor keeps its own copy through it).
    pub grid_version: Arc<AtomicU64>,
}

impl Default for BeatParams {
    fn default() -> Self {
        let p = DEFS
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let (min, max) = RANGES[k];
                IntParam::new(d.name, START[k], IntRange::Linear { min, max })
                    .with_value_to_string(Arc::new(move |v| value_text(k, v)))
                    .with_string_to_value(Arc::new(move |s| {
                        let s = s.trim().trim_end_matches('%').trim();
                        let (lo, hi) = RANGES[k];
                        if k == TIME_SLOT || k == VOL_SLOT {
                            return ["a", "b", "c", "d"].iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as i32);
                        }
                        if k == CYCLE {
                            // The cycle reads its own names back ("½ bar" through "4 bars") and a bare index.
                            return (0..4).find(|i| value_text(k, *i as i32).eq_ignore_ascii_case(s))
                                .or_else(|| s.parse::<f64>().ok().filter(|f| (0.0..=3.0).contains(f) && f.fract() == 0.0).map(|f| f as i32));
                        }
                        s.parse::<f64>().ok().map(|f| (f * 100.0).round().clamp(f64::from(lo), f64::from(hi)) as i32)
                    }))
            })
            .collect();
        BeatParams { p, grid: Arc::default(), live: Arc::default(), grid_version: Arc::default() }
    }
}

impl BeatParams {
    /// Setting `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    /// The settings the sound plays.
    pub fn settings(&self) -> dsp::Settings {
        dsp::Settings {
            time_mix: self.v(TIME_MIX) as f32 / 10000.0,
            vol_mix: self.v(VOL_MIX) as f32 / 10000.0,
            time_slot: self.v(TIME_SLOT).clamp(0, 3) as usize,
            vol_slot: self.v(VOL_SLOT).clamp(0, 3) as usize,
            cycle_beats: CYCLE_BEATS[self.v(CYCLE).clamp(0, 3) as usize],
        }
    }
}

unsafe impl Params for BeatParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        self.p.iter().zip(&DEFS).map(|(p, d)| (d.id.to_string(), p.as_ptr(), String::new())).collect()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(GRID_FIELD.to_string(), self.grid.load().to_json())])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(json) = serialized.get(GRID_FIELD) {
            match crate::grid::GridState::from_json(json) {
                Some(g) => {
                    self.grid.store(g);
                    self.grid_version.fetch_add(1, Ordering::Release);
                }
                None => nih_debug_assert_failure!("Mango Beat: the saved grids did not read"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts: the mixes in percent, the slots by letter, the cycle in bars; all parse back.
    #[test]
    fn value_texts_round_trip() {
        assert_eq!(value_text(TIME_MIX, 10000), "100.0%");
        assert_eq!(value_text(VOL_MIX, 0), "0.0%");
        assert_eq!(value_text(TIME_SLOT, 2), "C");
        assert_eq!(value_text(VOL_SLOT, 0), "A");
        assert_eq!(value_text(CYCLE, 0), "½ bar");
        assert_eq!(value_text(CYCLE, 3), "4 bars");
        let p = BeatParams::default();
        assert_eq!(p.settings(), dsp::Settings::default());
        assert_eq!(p.settings().cycle_beats, 16.0, "a new Mango Beat's cycle spans four bars");
        // A slot edit the host has not confirmed yet already reaches the sound.
        p.live.set(p.p[TIME_SLOT].as_ptr(), 0.9);
        assert_eq!(p.v(TIME_SLOT), 3);
        assert_eq!(p.settings().time_slot, 3);
    }
}
