// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Writes the factory presets (`crates/os-app/factory-presets/Mango Beat/*.ospreset`): one Mango Beat state
//! each. The presets are the plugin's own two lists — time presets and volume presets, Gross Beat's two banks —
//! and each state carries the preset's *recipe* (`tgen` / `vgen`) beside the grid it materialized into at the
//! default four-bar cycle, so the grid re-materializes when the cycle moves and the gates keep their per-beat
//! meanings at every cycle length. Run from the repository root:
//!
//! ```text
//! cargo run -p mango-beat --example presets
//! ```

use std::path::Path;

use mango_beat::grid::GridState;
use mango_beat::presets;

/// One preset to write: the name, the section's recipe ("" = leave the section at its default) and the other
/// section's recipe.
struct Out {
    name: &'static str,
    tgen: &'static str,
    vgen: &'static str,
}

/// The presets, in the plugin's two lists' order.
fn presets() -> Vec<Out> {
    let time = |name, recipe| Out { name, tgen: recipe, vgen: "" };
    let vol = |name, recipe| Out { name, tgen: "", vgen: recipe };
    let mut out = vec![];
    for p in presets::TIME_PRESETS {
        out.push(time(p.name, p.recipe));
    }
    for p in presets::VOL_PRESETS {
        out.push(vol(p.name, p.recipe));
    }
    // The combined one: half time under the classic trance gate.
    out.push(Out { name: "Half Time Gate", tgen: r#"{"k":"rate","r":0.5}"#, vgen: r#"{"k":"gate","spb":2.0}"# });
    out
}

/// The plugin's saved state: the parameters at their defaults, the grids materialized at the default four-bar
/// cycle and the recipes beside them.
fn state(tgen: &str, vgen: &str) -> Vec<u8> {
    const DEFAULT_CYCLE: f32 = 16.0;
    // The recipes travel in the gen form (slot 0, the recipe beside it).
    let tgen = if tgen.is_empty() { String::new() } else { presets::gen_for(0, tgen) };
    let vgen = if vgen.is_empty() { String::new() } else { presets::gen_for(0, vgen) };
    let mut g = GridState::default();
    if !tgen.is_empty() {
        if let Some((slot, recipe)) = presets::parse_gen(&tgen) {
            g.time[slot.min(3)] = presets::materialize(&recipe, DEFAULT_CYCLE).unwrap_or_default();
        }
    }
    if !vgen.is_empty() {
        if let Some((slot, recipe)) = presets::parse_gen(&vgen) {
            g.volume[slot.min(3)] = presets::materialize(&recipe, DEFAULT_CYCLE).unwrap_or_default();
        }
    }
    let mut fields = serde_json::json!({ "grid": g.to_json() });
    if !tgen.is_empty() {
        fields["tgen"] = serde_json::json!(tgen);
    }
    if !vgen.is_empty() {
        fields["vgen"] = serde_json::json!(vgen);
    }
    let json = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "params": {
            "time_mix": { "i32": 10000 },
            "vol_mix": { "i32": 10000 },
            "time_slot": { "i32": 0 },
            "vol_slot": { "i32": 0 },
            "cycle": { "i32": 3 },
        },
        "fields": fields,
    })
    .to_string();
    // The CLAP state form: the length in front of the JSON.
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/os-app/factory-presets/Mango Beat");
    std::fs::create_dir_all(&dir).expect("preset folder");
    for p in presets() {
        let header = format!("plugin=Mango Beat\tid=clap:org.openstudio.mango-beat\tformat=clap\n");
        let mut file = b"OSPRESET 1\n".to_vec();
        file.extend_from_slice(header.as_bytes());
        file.extend_from_slice(&state(p.tgen, p.vgen));
        let path = dir.join(format!("{}.ospreset", p.name));
        std::fs::write(&path, &file).expect("preset file");
        println!("wrote {}", path.display());
    }
}
