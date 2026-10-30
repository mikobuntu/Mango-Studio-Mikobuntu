// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mango Beat's factory presets: two lists, one per section — Gross Beat keeps its time and volume envelopes'
//! presets apart, and so does Mango Beat. A preset is a *recipe* (a little JSON), not a frozen grid: the sound's
//! cycle stretches and squeezes with the Cycle parameter, so the recipe is materialized for the cycle it plays
//! at — a trance gate is eighth notes whether the cycle is half a bar or four of them, and the editor re-
//! materializes when the cycle moves. A hand-drawn grid carries no recipe and stretches with the cycle as drawn.
//!
//! Recipes:
//! - time `{"k":"empty"}` — the section off (the live sound);
//! - time `{"k":"rate","r":R}` — a straight offset slope that plays at R times the sound's speed (R below one
//!   falls behind from the present, R above one climbs from the buffer's bottom);
//! - time `{"k":"tape"}` — a tape stop: the sound eases to a halt across the cycle;
//! - time `{"k":"scratch"}` — forward-backward runs at two beats a beat either way;
//! - volume `{"k":"gate","spb":S}` — S equal on-off steps a beat for the whole cycle;
//! - volume `{"k":"pump","per":P}` — a sidechain-style dip every P beats.

use serde_json::Value;

use crate::grid::{Curve, Mode, Pt, PANEL_BEATS};

/// One factory preset: its name and its recipe (the JSON `materialize` reads).
pub struct Preset {
    pub name: &'static str,
    pub recipe: &'static str,
}

/// The time section's presets (the playback choreography over the cycle).
pub const TIME_PRESETS: &[Preset] = &[
    Preset { name: "Default", recipe: r#"{"k":"empty"}"# },
    Preset { name: "Half Time", recipe: r#"{"k":"rate","r":0.5}"# },
    Preset { name: "Quarter Time", recipe: r#"{"k":"rate","r":0.25}"# },
    Preset { name: "Double Time", recipe: r#"{"k":"rate","r":2.0}"# },
    Preset { name: "Reverse", recipe: r#"{"k":"rate","r":-1.0}"# },
    Preset { name: "Tape Stop", recipe: r#"{"k":"tape"}"# },
    Preset { name: "Scratch", recipe: r#"{"k":"scratch"}"# },
];

/// The volume section's presets (the gate over the cycle).
pub const VOL_PRESETS: &[Preset] = &[
    Preset { name: "Default", recipe: r#"{"k":"empty"}"# },
    Preset { name: "Trance Gate 1-2", recipe: r#"{"k":"gate","spb":0.5}"# },
    Preset { name: "Trance Gate 1-4", recipe: r#"{"k":"gate","spb":1.0}"# },
    Preset { name: "Trance Gate 1-8", recipe: r#"{"k":"gate","spb":2.0}"# },
    Preset { name: "Trance Gate 1-16", recipe: r#"{"k":"gate","spb":4.0}"# },
    Preset { name: "Triplet Chop", recipe: r#"{"k":"gate","spb":3.0}"# },
    Preset { name: "Sidechain Pump", recipe: r#"{"k":"pump","per":4.0}"# },
];

/// A straight two-point line.
fn line(y0: f32, y1: f32) -> Curve {
    Curve { pts: vec![Pt::new(0.0, y0.clamp(0.0, 1.0), 0.0, Mode::Single as u8), Pt::new(1.0, y1.clamp(0.0, 1.0), 0.0, Mode::Single as u8)] }
}

/// Materializes a recipe for a cycle of `cycle_beats` beats; `None` when the recipe is not one.
pub fn materialize(recipe: &str, cycle_beats: f32) -> Option<Curve> {
    let v: Value = serde_json::from_str(recipe).ok()?;
    let cycle = cycle_beats.max(0.5);
    match v.get("k")?.as_str()? {
        // The section off.
        "empty" => Some(Curve::default()),
        // A straight offset slope at R times the sound's speed: the offset grows (1 - R) beats a beat, so the
        // well's y spans (1 - R) * cycle / 8 over the cycle. Speeds above one climb from the buffer's bottom,
        // since a live sound cannot be overtaken.
        "rate" => {
            let r = v.get("r").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            let span = (1.0 - r) * cycle / PANEL_BEATS;
            if span >= 0.0 {
                Some(line(0.0, span))
            } else if span >= -1.0 {
                Some(line(1.0, 1.0 + span))
            } else {
                // Faster than the well can give across the whole cycle: the slope runs until the head is back
                // at the present, then the sound plays live for the rest of the cycle.
                let stop = (-1.0 / span).min(1.0);
                Some(Curve {
                    pts: vec![
                        Pt::new(0.0, 1.0, 0.0, Mode::Single as u8),
                        Pt::new(stop, 0.0, 0.0, Mode::Single as u8),
                        Pt::new(1.0, 0.0, 0.0, Mode::Single as u8),
                    ],
                })
            }
        }
        // The tape stop: the offset's slope eases from the sound's own speed to a full stop, y = (cycle / 8) x^2
        // (the slope at the cycle's end is the pause angle), clamped to the well.
        "tape" => {
            let span = (cycle / PANEL_BEATS).min(1.0);
            let pts: Vec<Pt> = (0..=8)
                .map(|k| {
                    let x = k as f32 / 8.0;
                    Pt::new(x, span * x * x, 0.0, Mode::Single as u8)
                })
                .collect();
            Some(Curve { pts })
        }
        // The scratch: four forward and four backward runs at two beats a beat either way, centred in the well.
        "scratch" => {
            let amp = (cycle / PANEL_BEATS / 4.0).min(0.25);
            let base = (1.0 - amp) / 2.0;
            let mut pts = Vec::new();
            for k in 0..=8 {
                let x = k as f32 / 8.0;
                let y = if k % 2 == 0 { base } else { base + 2.0 * amp };
                pts.push(Pt::new(x, y, 0.0, Mode::Single as u8));
            }
            Some(Curve { pts })
        }
        // The gate: S on-off steps a beat across the whole cycle.
        "gate" => {
            let spb = v.get("spb").and_then(Value::as_f64).unwrap_or(2.0) as f32;
            Some(crate::dsp::gate(spb, cycle))
        }
        // The pump: a dip and a smooth climb back, every `per` beats across the cycle.
        "pump" => {
            let per = v.get("per").and_then(Value::as_f64).unwrap_or(4.0) as f32;
            let dips = (cycle / per.max(0.5)).round().max(1.0) as usize;
            let w = 1.0 / dips as f32;
            let mut pts = Vec::new();
            for k in 0..dips {
                let x0 = k as f32 * w;
                pts.push(Pt::new(x0, 1.0, -0.4, Mode::Single as u8));
                pts.push(Pt::new(x0 + 0.3 * w, 0.45, 0.0, Mode::Smooth as u8));
                pts.push(Pt::new(x0 + w, 1.0, 0.0, Mode::Smooth as u8));
            }
            let mut c = Curve { pts };
            c.normalize();
            Some(c)
        }
        _ => None,
    }
}

/// A recipe's `k` ("" when the text is not one).
pub fn kind(recipe: &str) -> String {
    serde_json::from_str::<Value>(recipe)
        .ok()
        .and_then(|v| v.get("k").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

/// The gen state field's shape: `{"s": slot, "r": recipe}` — the slot the preset sits in and its recipe text.
pub fn parse_gen(field: &str) -> Option<(usize, String)> {
    let v: Value = serde_json::from_str(field).ok()?;
    Some((v.get("s")?.as_u64()? as usize, v.get("r")?.as_str()?.to_string()))
}

/// The gen state field for a slot and a recipe (what the editor saves when a preset is picked).
pub fn gen_for(slot: usize, recipe: &str) -> String {
    serde_json::json!({ "s": slot.min(3), "r": recipe }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridState;

    /// The rate recipes land on the speeds they name: half time at every cycle length, double time wherever the
    /// well can give two beats a beat of catch-up (up to a two-bar cycle), and past that the slope runs to the
    /// present and holds it.
    #[test]
    fn rate_recipes_keep_their_speed_at_every_cycle() {
        for cycle in [2.0f32, 4.0, 8.0, 16.0] {
            let c = materialize(r#"{"k":"rate","r":0.5}"#, cycle).unwrap();
            let span = GridState::offset_of_y(c.pts[1].y);
            let got = 1.0 - span / cycle;
            assert!((got - 0.5).abs() < 1e-4, "cycle {cycle}: half time plays at {got}");
        }
        for cycle in [2.0f32, 4.0, 8.0] {
            let c = materialize(r#"{"k":"rate","r":2.0}"#, cycle).unwrap();
            let span = GridState::offset_of_y(c.pts[0].y) - GridState::offset_of_y(c.pts[1].y);
            assert!((1.0 + span / cycle - 2.0).abs() < 1e-4, "cycle {cycle}: double time plays at {}", 1.0 + span / cycle);
        }
        let c = materialize(r#"{"k":"rate","r":2.0}"#, 16.0).unwrap();
        assert_eq!(c.pts.len(), 3, "double time over four bars: the slope, then live for the rest");
        let span = GridState::offset_of_y(c.pts[0].y) - GridState::offset_of_y(c.pts[1].y);
        assert!((1.0 + span / 8.0 - 2.0).abs() < 1e-4, "the slope's stretch plays at two");
    }

    /// The gate recipes keep their per-beat step counts at every cycle length: that is the point of the recipes.
    #[test]
    fn gate_recipes_keep_their_per_beat_timing() {
        for cycle in [2.0f32, 4.0, 8.0, 16.0] {
            let c = materialize(r#"{"k":"gate","spb":2.0}"#, cycle).unwrap();
            assert_eq!(c.pts.len(), (2.0 * cycle) as usize * 2, "cycle {cycle}: two on-off steps a beat");
        }
    }

    /// The pump dips once a bar whatever the cycle; the tape stop lands stopped; the scratch stays in the well.
    #[test]
    fn shape_recipes_span_the_cycle() {
        let p = materialize(r#"{"k":"pump","per":4.0}"#, 16.0).unwrap();
        assert_eq!(p.pts.len(), 12, "four dips over the four-bar cycle");
        let p = materialize(r#"{"k":"pump","per":4.0}"#, 4.0).unwrap();
        assert_eq!(p.pts.len(), 3, "one dip over the one-bar cycle");
        let t = materialize(r#"{"k":"tape"}"#, 8.0).unwrap();
        assert!((t.pts[8].y - 1.0).abs() < 1e-5, "the tape stops at the well's bottom over two bars");
        let t = materialize(r#"{"k":"tape"}"#, 16.0).unwrap();
        assert!((t.pts[8].y - 1.0).abs() < 1e-5, "the tape clamps to the well over four bars");
        let s = materialize(r#"{"k":"scratch"}"#, 16.0).unwrap();
        assert!(s.pts.iter().all(|p| (0.0..=1.0).contains(&p.y)));
        assert!(materialize("not a recipe", 4.0).is_none());
        assert_eq!(kind(r#"{"k":"gate","spb":2.0}"#), "gate");
        assert_eq!(kind("junk"), "");
    }
}
