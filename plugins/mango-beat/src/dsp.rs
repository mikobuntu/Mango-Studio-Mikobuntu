// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mango Beat's sound: Gross Beat's time section and a gate, both driven by the envelope grids.
//!
//! The time section keeps the input in a ring and reads it back where the time grid says, Gross Beat's way: the
//! grid is an offset in beats back from the live sound, so a horizontal line plays at the sound's own speed (a
//! plain delay), a down slope falls behind the present — at one beat a beat it pauses, past that angle it walks
//! backwards — and a Hold step repositions the read head at once, which is the beat-repeat jump. The volume
//! section multiplies the sound by the volume grid, smoothed a half millisecond so gates do not click. With no
//! grid in a section the section is transparent, so a fresh Mango Beat passes the sound through untouched.
//!
// spec: plugins/mango-beat.md (§3 sound)

use crate::grid::{GridState, Mode, Pt};

/// The history's length per channel, in frames: a power of two (2^20, about 21 s at 44.1 kHz; 8 MB of ring).
const HISTORY: usize = 1 << 20;
/// The read head keeps this far behind the write head at the least (the interpolator reads one older frame) and
/// the most (past it the sound has run out of past and stalls until the rate picks up).
const MIN_D: f64 = 1.0;
const MAX_D: f64 = (HISTORY - 2) as f64;
/// The gate's de-click: the smoothing time in seconds (a half millisecond a gate edge takes to close).
const SMOOTH_S: f32 = 0.0005;

/// The transport position a block starts at (`playing` and the song's beats); `None` plays nothing new.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportStep {
    pub playing: bool,
    pub beats: f64,
}

/// The settings the sound takes from the parameters and the grids, all at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The time section's mix (0..1): dry sound against the re-read.
    pub time_mix: f32,
    /// The volume section's mix (0..1): the sound against the gated sound.
    pub vol_mix: f32,
    /// The time slot (0..4) and the volume slot (0..4) playing.
    pub time_slot: usize,
    pub vol_slot: usize,
    /// The envelope cycle's length in beats (2, 4, 8 or 16: half a bar through four bars of 4/4). Both grids'
    /// curves span one cycle; a half-time passage can now stretch across four bars before the grid repeats.
    pub cycle_beats: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { time_mix: 1.0, vol_mix: 1.0, time_slot: 0, vol_slot: 0, cycle_beats: 16.0 }
    }
}

/// One Mango Beat voice of sound.
pub struct Beat {
    rate: u32,
    bpm: f64,
    settings: Settings,
    grids: GridState,
    /// The ring: the past of the sound.
    hist_l: Vec<f32>,
    hist_r: Vec<f32>,
    /// The next write position in the ring.
    write: usize,
    /// The read head's distance behind the write head, in frames: the time grid's offset, directly.
    d: f64,
    /// Where in the envelope cycle the envelopes are (0..1; the cycle is `Settings::cycle_beats` beats long).
    phase: f64,
    /// The transport: playing, and the beats the next block should start at (jumps reposition the envelopes).
    playing: bool,
    next_beats: Option<f64>,
    /// The smoothed gate gain.
    gain: f32,
    /// The smoothing coefficient from the rate (made in `set_sample_rate`).
    smooth_k: f32,
}

impl Default for Beat {
    fn default() -> Self {
        Beat::new(44100)
    }
}

impl Beat {
    pub fn new(rate: u32) -> Beat {
        let mut b = Beat {
            rate: 44100,
            bpm: 130.0,
            settings: Settings::default(),
            grids: GridState::default(),
            hist_l: vec![0.0; HISTORY],
            hist_r: vec![0.0; HISTORY],
            write: 0,
            d: MIN_D,
            phase: 0.0,
            playing: false,
            next_beats: None,
            gain: 1.0,
            smooth_k: 0.0,
        };
        b.set_sample_rate(rate);
        b
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
        self.smooth_k = 1.0 - f32::exp(-1.0 / (SMOOTH_S * self.rate as f32));
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 {
            self.bpm = bpm;
        }
    }

    pub fn set(&mut self, s: &Settings) {
        self.settings = *s;
    }

    /// The cycle's length in beats, kept sane whatever a project's state carries.
    fn cycle(&self) -> f64 {
        f64::from(self.settings.cycle_beats.clamp(0.5, 64.0))
    }

    /// The grids moved (the editor edited, a preset or a project loaded): the sound plays them from the next block.
    pub fn set_grids(&mut self, g: GridState) {
        self.grids = g;
    }

    /// Empties the history and the heads: the next block starts from nothing.
    pub fn reset(&mut self) {
        self.hist_l.fill(0.0);
        self.hist_r.fill(0.0);
        self.write = 0;
        self.d = MIN_D;
        self.phase = 0.0;
        self.next_beats = None;
        self.gain = 1.0;
    }

    /// The offset the time grid asks for at `phase`, in panel units 0..1 (0 is the live sound, 1 is
    /// [`crate::grid::PANEL_BEATS`] beats back): an empty grid is the live sound.
    fn offset_at(&self, phase: f64) -> f32 {
        self.grids.time[self.settings.time_slot.min(3)].value_at(phase).unwrap_or(0.0)
    }

    /// The gain the volume grid asks for at `phase`: an empty grid is wide open.
    pub(crate) fn gain_at(&self, phase: f64) -> f32 {
        match self.grids.volume[self.settings.vol_slot.min(3)].value_at(phase) {
            Some(y) => y.clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// One block, in place, planar stereo. `transport` is the host's position at the block's start (`None`: the
    /// host says nothing, the envelopes free-run).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], transport: Option<TransportStep>) {
        let frames = l.len().min(r.len());
        let Some(ts) = transport else { return self.process_free(l, r, frames) };
        let bps = if self.bpm > 0.0 { self.bpm / 60.0 / f64::from(self.rate) } else { 0.0 };
        if ts.playing {
            self.playing = true;
            self.next_beats = Some(ts.beats + frames as f64 * bps);
            // The envelopes follow the host's beats through the cycle: beat 0 of the song and every `cycle` beats
            // after it land at the cycle's start.
            let cycle = self.cycle();
            self.run(l, r, frames, ts.beats.rem_euclid(cycle) / cycle, bps);
        } else {
            self.playing = false;
            self.next_beats = None;
            self.run(l, r, frames, self.phase, bps);
        }
    }

    /// Without a transport word the envelopes keep running on their own (the tail after a stop).
    fn process_free(&mut self, l: &mut [f32], r: &mut [f32], frames: usize) {
        let bps = if self.bpm > 0.0 { self.bpm / 60.0 / f64::from(self.rate) } else { 0.0 };
        self.run(l, r, frames, self.phase, bps);
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32], frames: usize, mut phase: f64, bps: f64) {
        let mask = HISTORY - 1;
        let cycle = self.cycle();
        let (tm, vm) = (self.settings.time_mix.clamp(0.0, 1.0), self.settings.vol_mix.clamp(0.0, 1.0));
        let time_on = !self.grids.time[self.settings.time_slot.min(3)].pts.is_empty() && tm > 0.0;
        let vol_on = !self.grids.volume[self.settings.vol_slot.min(3)].pts.is_empty() && vm > 0.0;
        for k in 0..frames {
            // The envelopes at this frame (a frame is `bps` beats, the cycle is `cycle` beats long).
            if bps > 0.0 {
                phase += bps / cycle;
                phase = phase.rem_euclid(1.0);
            }
            let target = if vol_on { self.gain_at(phase) } else { 1.0 };
            // The gate glides to its target so steps do not click.
            self.gain += (target - self.gain) * self.smooth_k;
            let (il, ir) = (l[k], r[k]);
            // The past keeps growing whatever the sections do, so switching a section on mid-sound has material.
            self.hist_l[self.write] = il;
            self.hist_r[self.write] = ir;
            self.write = (self.write + 1) & mask;
            // The read head sits where the grid says, Gross Beat's way: the grid is an offset in beats back from
            // the live sound, so a horizontal line plays at the sound's own speed, a down slope falls behind (at
            // the pause angle it stops, steeper it walks backwards) and a Hold step repositions the head at once.
            // Past the ring's depth the head stalls at the buffer's bottom, as Gross Beat's two-bar buffer does.
            if time_on && bps > 0.0 {
                let offset = f64::from(self.offset_at(phase)) * f64::from(crate::grid::PANEL_BEATS);
                self.d = (offset / bps).clamp(MIN_D, MAX_D);
            } else if !time_on {
                self.d = MIN_D;
            }
            let (mut out_l, mut out_r) = if time_on {
                let j = self.d.floor() as usize;
                let f = (self.d - j as f64) as f32;
                let i0 = (self.write.wrapping_sub(j)) & mask;
                let i1 = (i0 + HISTORY - 1) & mask;
                let w0 = 1.0 - f;
                (
                    self.hist_l[i0] * w0 + self.hist_l[i1] * f,
                    self.hist_r[i0] * w0 + self.hist_r[i1] * f,
                )
            } else {
                (il, ir)
            };
            if vol_on {
                out_l *= self.gain;
                out_r *= self.gain;
            }
            // The sections' mixes.
            l[k] = il + (out_l - il) * tm;
            r[k] = ir + (out_r - ir) * vm;
        }
        self.phase = phase;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Presets' grids (shared with the factory preset generator)

/// A gate grid of `steps` equal on-off steps per beat across a `cycle_beats`-beat cycle (`gate(2.0, 16.0)` is
/// the classic eighth-note trance gate over the four-bar default cycle; `gate(1.0, ..)` gates quarter notes).
pub fn gate(steps: f32, cycle_beats: f32) -> crate::grid::Curve {
    let hold = Mode::Hold as u8;
    let total = ((steps.max(0.125) * cycle_beats.max(0.5)).round() as usize).max(1);
    let step = 1.0 / total as f32;
    let mut pts = Vec::new();
    for k in 0..total {
        pts.push(Pt::new(step * k as f32, 1.0, 0.0, hold));
        pts.push(Pt::new(step * (k as f32 + 0.5), 0.0, 0.0, hold));
    }
    crate::grid::Curve { pts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Curve, GridState};

    const RATE: u32 = 48000;

    fn beat() -> Beat {
        let mut b = Beat::new(RATE);
        b.set_tempo(120.0);
        b
    }

    /// A fresh Mango Beat with no grids is the sound itself, sample for sample.
    #[test]
    fn empty_grids_pass_the_sound_through() {
        let mut b = beat();
        let mut l: Vec<f32> = (0..256).map(|i| (i as f32 * 0.013).sin()).collect();
        let mut r: Vec<f32> = (0..256).map(|i| (i as f32 * 0.029).sin()).collect();
        let (il, ir) = (l.clone(), r.clone());
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        assert_eq!(l, il);
        assert_eq!(r, ir);
    }

    /// A horizontal time line delays the sound by the offset it sits at, at the sound's own speed: a line one
    /// beat down (an eighth of the well) is a one-beat delay.
    #[test]
    fn a_horizontal_line_delays_the_sound() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 0.125, 0.0, Mode::Single as u8)] };
        b.set_grids(g);
        let n = 24000; // a beat at 120 bpm and 48 kHz
        let mut l: Vec<f32> = (0..2 * n).map(|i| i as f32 / (2 * n) as f32).collect();
        let mut r = vec![0.0; 2 * n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // Past the delay the output is the input from a beat ago.
        let k = n + 5000;
        let want = (k - n) as f32 / (2 * n) as f32;
        assert!((l[k] - want).abs() < 0.01, "one beat behind: {} vs {want}", l[k]);
    }

    /// Half time: a slope that falls behind at half a beat a beat plays the past at half the sound's speed.
    #[test]
    fn half_time_plays_half_as_fast() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let mut g = GridState::default();
        // Half speed's offset slope over a one-bar cycle: (1 - 0.5) * 4 beats over the well's 8 = a quarter down.
        g.time[0] = Curve {
            pts: vec![Pt::new(0.0, 0.0, 0.0, Mode::Single as u8), Pt::new(1.0, 0.25, 0.0, Mode::Single as u8)],
        };
        b.set_grids(g);
        let n = 24000;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // The output rises at about half the input's slope.
        let slope = (l[400] - l[300]) / 100.0;
        assert!(slope > 0.000012 && slope < 0.000032, "slope {slope}");
    }

    /// Double time catches up with the past: a slope rising a beat a beat plays at twice the sound's speed (a
    /// live sound itself cannot be overtaken, so the slope burns the backlog it is given).
    #[test]
    fn double_time_plays_twice_as_fast() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let n = 24000; // a beat
        // Two beats play with the head held two beats down (the backlog builds in the ring).
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 0.25, 0.0, Mode::Single as u8)] };
        b.set_grids(g);
        let mut l: Vec<f32> = (0..2 * n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; 2 * n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // Then the catch-up slope, entered where the head sits (two beats down at the cycle's half): from there
        // to the present over the second half of the cycle is a beat a beat of catch-up, twice the speed.
        let mut g = GridState::default();
        g.time[0] = Curve {
            pts: vec![Pt::new(0.0, 0.5, 0.0, Mode::Single as u8), Pt::new(1.0, 0.0, 0.0, Mode::Single as u8)],
        };
        b.set_grids(g);
        let mut l: Vec<f32> = (2 * n..4 * n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; 2 * n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 2.0 }));
        // Measure deep in the block, where the slope is in its steady two-times climb.
        let slope = (l[3000] - l[1000]) / 2000.0;
        assert!(slope > 0.000065 && slope < 0.000105, "slope {slope}");
    }

    /// Reverse: a slope falling two beats a beat walks the read head backwards at the sound's own speed, once
    /// there is a past to walk through.
    #[test]
    fn a_reverse_slope_plays_the_past_backwards() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let n = 24000; // a beat
        // A beat of ramp plays live first (an empty grid is the present).
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // Then the reverse slope, entered at the cycle's start: the head walks back through the ramp.
        let mut g = GridState::default();
        g.time[0] = Curve {
            pts: vec![Pt::new(0.0, 0.0, 0.0, Mode::Single as u8), Pt::new(1.0, 1.0, 0.0, Mode::Single as u8)],
        };
        b.set_grids(g);
        let mut l = vec![1.0f32; n];
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 4.0 }));
        let slope = (l[2500] - l[1000]) / 1500.0;
        assert!(slope < -0.000005, "the output descends through the past: {slope}");
    }

    /// A Hold step that jumps the offset down repositions the head at once: the classic half-bar repeat.
    #[test]
    fn a_hold_step_repeats_the_past() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let mut g = GridState::default();
        // Live for half the cycle, then a step two beats back: the first half of the bar plays twice.
        g.time[0] = Curve {
            pts: vec![
                Pt::new(0.0, 0.0, 0.0, Mode::Hold as u8),
                Pt::new(0.5, 0.0, 0.0, Mode::Hold as u8),
                Pt::new(0.5, 0.25, 0.0, Mode::Hold as u8),
            ],
        };
        b.set_grids(g);
        let n = 96000; // the whole cycle
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // Three quarters in the head sits two beats back: the output is the sound of half a cycle ago.
        let k = 72000;
        let want = (k - 48000) as f32 / n as f32;
        assert!((l[k] - want).abs() < 0.01, "reading two beats back: {} vs {want}", l[k]);
    }

    /// The head sits where the grid says after a transport jump: the host's beats map into the cycle.
    #[test]
    fn the_head_sits_where_the_grid_says() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 0.25, 0.0, Mode::Single as u8)] };
        b.set_grids(g);
        let n = 256;
        let mut l = vec![0.5; n];
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 10.0 }));
        // The grid holds a quarter of the well down: two beats back, 48000 frames at this tempo.
        assert!((b.d - 48000.0).abs() < 1.0, "two beats back after the jump: {}", b.d);
    }

    /// A gate closes and opens where its points say, smoothly (no step wider than the smoothing); the cycle is
    /// pinned to one beat so a point at 0.5 sits half a beat in.
    #[test]
    fn the_gate_closes_where_its_points_say() {
        let mut b = beat();
        b.set(&Settings { cycle_beats: 1.0, ..Settings::default() });
        let mut g = GridState::default();
        g.volume[0] = Curve { pts: vec![Pt::new(0.0, 1.0, 0.0, Mode::Hold as u8), Pt::new(0.5, 0.0, 0.0, Mode::Hold as u8)] };
        b.set_grids(g);
        let n = 1024; // 120 bpm at 48 kHz: one beat is 24000 frames; 1024 frames is 4.3 % of the beat
        let mut l = vec![1.0; n];
        let mut r = vec![1.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        // The whole block sits in the open half: nothing was gated yet.
        assert!(l.iter().all(|v| *v > 0.99));
        // From beat 0.5 on the gate is closed: start a block there.
        let mut l = vec![1.0; n];
        let mut r = vec![1.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.5 }));
        assert!(*l.last().unwrap() < 0.01, "closed by the end: {}", l.last().unwrap());
    }

    /// The tail free-runs without a transport word and stays finite (a stopped host still drains the ring).
    #[test]
    fn the_tail_free_runs_and_stays_finite() {
        let mut b = beat();
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 0.125, 0.0, Mode::Single as u8)] };
        g.volume[0] = gate(2.0, 16.0);
        b.set_grids(g);
        let n = 256;
        let mut l = vec![0.5; n];
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        for _ in 0..8 {
            let mut l = vec![0.0; n];
            let mut r = vec![0.0; n];
            b.process(&mut l, &mut r, None);
            assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        }
    }

    /// Random grids at random phases never leave the output range or make a NaN.
    #[test]
    fn random_grids_stay_finite() {
        let mut b = beat();
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 40) as f32) / (1u32 << 24) as f32
        };
        let mut g = GridState::default();
        for slot in 0..4 {
            for (sec, hi) in [(0, 1.0f32), (1, 1.0)] {
                let n = 1 + (rand() * 5.0) as usize;
                let pts: Vec<Pt> = (0..n).map(|k| Pt::new(k as f32 / n as f32, rand() * hi, rand() * 2.0 - 1.0, (rand() * 13.0) as u8)).collect();
                let mut c = Curve { pts };
                c.normalize();
                if sec == 0 {
                    g.time[slot] = c;
                } else {
                    g.volume[slot] = c;
                }
            }
        }
        b.set_grids(g);
        let n = 2048;
        let mut l: Vec<f32> = (0..n).map(|_| rand() * 2.0 - 1.0).collect();
        let mut r = l.clone();
        for k in 0..4 {
            let beats = f64::from(k) * 0.37 + 0.11;
            b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats }));
            assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 1.001), "frame range at {k}");
            assert!(r.iter().all(|v| v.is_finite()));
        }
    }

    /// The mixes: a time mix of 0 leaves the timing alone even with a grid; a volume mix of 0 leaves the level.
    #[test]
    fn the_mixes_fade_their_sections_out() {
        let mut b = beat();
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 0.25, 0.0, Mode::Single as u8)] };
        g.volume[0] = gate(2.0, 16.0);
        b.set_grids(g);
        b.set(&Settings { time_mix: 0.0, vol_mix: 0.0, ..Settings::default() });
        let n = 256;
        let mut l: Vec<f32> = vec![0.6; n];
        let mut r = vec![0.0; n];
        let (il, ir) = (l.clone(), r.clone());
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        assert_eq!(l, il);
        assert_eq!(r, ir);
    }

    /// A gate of two equal steps makes the eighth-note pattern the classic trance gate rides on (over a one-beat
    /// cycle), and the cycle stretches the same pattern across its length.
    #[test]
    fn gate_steps_alternate() {
        let g = gate(2.0, 1.0);
        assert_eq!(g.pts.len(), 4);
        assert!((g.value_at(0.1).unwrap() - 1.0).abs() < 1e-6, "open on the first half of the step");
        assert_eq!(g.value_at(0.3), Some(0.0), "closed on the second half");
        assert_eq!(gate(2.0, 16.0).pts.len(), 64, "two steps a beat over the four-bar default: 32 on-off pairs");
    }

    /// The cycle stretches the envelopes: with the four-bar default, a gate that closes at the cycle's midpoint
    /// stays open through beat 7.9 and is closed by beat 8.1 (the host's beats map into the cycle).
    #[test]
    fn the_cycle_stretches_the_envelopes() {
        let mut b = beat();
        let mut g = GridState::default();
        g.volume[0] = Curve { pts: vec![Pt::new(0.0, 1.0, 0.0, Mode::Hold as u8), Pt::new(0.5, 0.0, 0.0, Mode::Hold as u8)] };
        b.set_grids(g);
        let n = 1024;
        // Beat 7.9: the block ends at phase 0.4964, still the cycle's open first half.
        let mut l = vec![1.0; n];
        let mut r = vec![1.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 7.9 }));
        assert!(l.iter().all(|v| *v > 0.99), "open before the cycle's midpoint");
        // Beat 8.1: past the midpoint the gate shuts (the smoothing takes half a millisecond).
        let mut l = vec![1.0; n];
        let mut r = vec![1.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 8.1 }));
        assert!(*l.last().unwrap() < 0.01, "closed after the cycle's midpoint: {}", l.last().unwrap());
    }

    /// Slower than the buffer: at an absurd tempo the head stalls at the ring's bottom and stays finite, as
    /// Gross Beat's two-bar buffer runs out.
    #[test]
    fn slow_tempos_stall_at_the_buffer_bottom() {
        let mut b = beat();
        b.set_tempo(1.0);
        b.set(&Settings { cycle_beats: 4.0, ..Settings::default() });
        let mut g = GridState::default();
        g.time[0] = Curve { pts: vec![Pt::new(0.0, 1.0, 0.0, Mode::Single as u8)] };
        b.set_grids(g);
        let n = 256;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let mut r = vec![0.0; n];
        b.process(&mut l, &mut r, Some(TransportStep { playing: true, beats: 0.0 }));
        assert!((b.d - MAX_D).abs() < 1.0, "the head stalled at the buffer's bottom: {}", b.d);
        assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    }
}
