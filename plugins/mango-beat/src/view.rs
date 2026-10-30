// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mango Beat editor: painting and hit regions (platform independent). Two cycle grids — TIME over VOLUME —
//! drawn like the playlist draws automation (a filled curve over ruled beats and bars, points and tension
//! handles), with the sections' slot buttons and mix dials in the rail at the left, the hint well and the
//! wordmark along the foot.
//!
// spec: plugins/mango-beat.md (§1 window, §2 controls, §3 hints)

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State};
use os_ui::{Canvas, Color, Font, Rect};

use crate::dsp;
use crate::grid::{Curve, GridState, Mode};

/// Editor size in logical pixels.
pub const W: u32 = 760;
pub const H: u32 = 470;

/// The sections: the time grid (the offset back from the live sound) and the volume grid (the gate).
pub const TIME: usize = 0;
pub const VOL: usize = 1;

/// The two grid wells (the plot areas).
pub const GRIDS: [Rect; 2] = [Rect::new(140.0, 30.0, 610.0, 180.0), Rect::new(140.0, 238.0, 610.0, 180.0)];
/// The rail cards behind the slot buttons and the mix dials.
pub const RAIL: [Rect; 2] = [Rect::span(8.0, 130.0, 30.0, 210.0), Rect::span(8.0, 130.0, 238.0, 418.0)];
/// The slot buttons: two columns of two, per section.
pub const SLOT_TOP: f32 = 56.0;
pub const SLOT_LEFT: [f32; 2] = [22.0, 72.0];
pub const SLOT_SIZE: f32 = 24.0;
/// The mix dial's centre and radius.
pub const MIX_CX: f32 = 69.0;
pub const MIX_CYS: [f32; 2] = [158.0, 366.0];
pub const MIX_R: f32 = 20.0;

/// The cycle picker in the band between the wells (it governs both grids).
pub const CYCLE_Y: f32 = 214.0;
pub const CYCLE_H: f32 = 20.0;
pub const CYCLE_BW: f32 = 56.0;

/// The foot's centre line: the hint well at the left and the wordmark at the right.
const FOOT: f32 = 446.0;
const RIGHT: f32 = 750.0;

const RULE: Color = Color::rgb(0x3c3c3c);
const HEADING: Font = Font::condensed(11, 0).bold();
const LABEL: Font = Font::new(10).bold();
const MARK: Font = Font::condensed(9, 0);
const HINT: Font = Font::new(11);
const MARK_BIG: Font = Font::condensed(22, 0).bold();
const MARK_SMALL: Font = Font::condensed(10, 0).bold();

const CURVE_FILL: Color = Color::rgb(0xe8912d);
/// The curve and points of the section whose grid is being edited.
const CURVE_INK: Color = Color::rgb(0xf2a955);
/// The idle section draws dimmer.
const CURVE_DIM: Color = Color::rgb(0x8a6a45);
const POINT_BG: Color = Color::rgb(0x101010);

/// Read access to parameter values by id.
pub trait Model {
    fn norm(&self, id: &str) -> f32;
    fn plain(&self, id: &str) -> f32;
    fn text(&self, id: &str) -> String;
}

impl Model for os_plugkit::vals::Vals {
    fn norm(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::norm(self, id)
    }
    fn plain(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::plain(self, id)
    }
    fn text(&self, id: &str) -> String {
        os_plugkit::vals::Vals::text(self, id)
    }
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel; menu on right click.
    Knob(String),
    /// A section's slot button (click picks the slot; menu on right click).
    Slot(usize, usize),
    /// A cycle button in the band between the wells (click picks the length; menu on right click).
    Cycle(usize),
    /// An envelope point: section, slot, point (drag moves it; menu on right click).
    Point(usize, usize, usize),
    /// A grid well away from its points: a click plants a point there.
    Grid(usize),
    /// The wordmark (the About menu).
    Logo,
}

impl Target {
    /// The setting's id ("" for the regions without one).
    pub fn id(&self) -> &str {
        match self {
            Target::Knob(id) => id,
            _ => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The pointer, the pressed control and the point being drawn active.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    /// The point drawn active (the last touched).
    pub active: Option<(usize, usize, usize)>,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
    m: &'a dyn Model,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn state(&self, r: Rect, t: &Target, disabled: bool) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, disabled, ..State::NORMAL }
    }
}

/// The x on a grid for a beat position (0..1).
pub fn gx(sec_grid: usize, x: f32) -> f32 {
    let g = &GRIDS[sec_grid];
    g.x + g.w * x.clamp(0.0, 1.0)
}

/// The y on a grid for an envelope value (0..1): the volume well has up = more, the time well has DOWN =
/// further into the past (Gross Beat's panel: the top edge is the live sound).
pub fn gy(sec_grid: usize, y: f32) -> f32 {
    let g = &GRIDS[sec_grid];
    if sec_grid == TIME {
        g.y + g.h * y.clamp(0.0, 1.0)
    } else {
        g.bottom() - g.h * y.clamp(0.0, 1.0)
    }
}

/// The beat position and envelope value at a point on a grid.
pub fn value_at(sec_grid: usize, x: f32, y: f32) -> (f32, f32) {
    let g = &GRIDS[sec_grid];
    let v = if sec_grid == TIME { (y - g.y) / g.h } else { (g.bottom() - y) / g.h };
    (((x - g.x) / g.w).clamp(0.0, 1.0), v.clamp(0.0, 1.0))
}

/// The cycle the wells rule themselves by, from the model.
pub fn cycle_of(m: &dyn Model) -> f32 {
    crate::params::CYCLE_BEATS[(m.plain("cycle").round() as i32).clamp(0, 3) as usize]
}

/// An offset's text on the time grid ("live", "1 beat", "1 bar", "1.5 beats", "2 bars").
pub fn offset_text(offset: f32) -> String {
    let o = offset.max(0.0);
    if o < 1e-3 {
        "live".into()
    } else if (o - 4.0).abs() < 1e-3 {
        "1 bar".into()
    } else if (o - 8.0).abs() < 1e-3 {
        "2 bars".into()
    } else if (o - o.round()).abs() < 1e-3 {
        match o.round() as i32 {
            1 => "1 beat".into(),
            n => format!("{n} beats"),
        }
    } else {
        format!("{o:.1} beats")
    }
}

/// The gate's value as decibels ("-inf dB" at 0).
pub fn db_text(y: f32) -> String {
    if y <= 1e-4 {
        "-inf dB".into()
    } else {
        format!("{:.1} dB", 20.0 * (y.max(1e-4) as f64).log10())
    }
}

/// The slot's letter.
pub fn slot_name(slot: usize) -> &'static str {
    ["A", "B", "C", "D"][slot.min(3)]
}

/// Paints the editor; `grids` are the envelopes, `hint` (empty when the host shows it) goes in the well at the foot.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, grids: &GridState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    // The rail cards and their headings.
    for (sec, r) in RAIL.iter().enumerate() {
        lemon::plug_panel(ui.c, *r);
        let name = if sec == TIME { "TIME" } else { "VOLUME" };
        heading(&mut ui, r.x, r.right(), r.y - 12.0, name);
    }
    // The two grids and their curves.
    for (sec, g) in GRIDS.iter().enumerate() {
        grid_well(&mut ui, sec, g);
        ui.hit(*g, Target::Grid(sec));
        let slot = ui.slot_of(sec).min(3);
        let curve = if sec == TIME { &grids.time[slot] } else { &grids.volume[slot] };
        let active = st.active.is_some_and(|(s, sl, _)| s == sec && sl == slot);
        let ink = if active { CURVE_INK } else { CURVE_DIM };
        let editing = st.active.is_some_and(|(s, _, _)| s == sec);
        curve_shape(&mut ui, sec, curve, ink, editing);
        points(&mut ui, sec, slot, curve, active);
    }
    // The rail: the slots, the mix dials.
    for (sec, mix_id) in [(TIME, "time_mix"), (VOL, "vol_mix")] {
        for i in 0..4 {
            slot_button(&mut ui, sec, i);
        }
        knob(&mut ui, mix_id, MIX_CX, MIX_CYS[sec], MIX_R, "MIX", false);
    }
    // The cycle both grids ride on.
    cycle_picker(&mut ui);
    // The foot.
    let mark_left = logo(&mut ui);
    let well = Rect::span(8.0, mark_left - 16.0, FOOT - 11.0, FOOT + 11.0);
    lemon::plug_well(ui.c, well, 6.0);
    if !hint.is_empty() {
        let lh = ui.c.line_height(HINT);
        ui.c.text_clip(HINT, well.x + 8.0, (well.cy() - lh / 2.0).round(), well.w - 16.0, brand::TEXT, hint);
    }
    ui.hits
}

/// The section's slot the parameters play (by the slot parameter's id).
impl Ui<'_> {
    fn slot_of(&self, sec: usize) -> usize {
        let id = if sec == TIME { "time_slot" } else { "vol_slot" };
        (self.m.plain(id).round() as i32).clamp(0, 3) as usize
    }
}

/// A card's heading: letter-spaced caps centred on a hairline rule at `y`.
fn heading(ui: &mut Ui, x0: f32, x1: f32, y: f32, s: &str) {
    let lh = ui.c.line_height(HEADING);
    let w = lemon::spaced_width(ui.c, HEADING, s, 2.0);
    let tx = ((x0 + x1 + 1.0) / 2.0 - w / 2.0).round();
    lemon::spaced(ui.c, HEADING, tx, (y - lh / 2.0).round(), plug::DIM, s, 2.0);
    if tx - 7.0 > x0 + 8.0 {
        ui.c.fill_rect(Rect::span(x0 + 4.0, tx - 7.0, y, y + 1.0), RULE);
        ui.c.fill_rect(Rect::span((tx + w + 6.0).round(), x1 - 4.0, y, y + 1.0), RULE);
    }
}

/// One grid well: the cycle's beats ruled (bars strong, beats medium, quarter-beats faint when they fit), the
/// value guides and their labels.
fn grid_well(ui: &mut Ui, sec: usize, g: &Rect) {
    lemon::plug_well(ui.c, g.outset(1.0), 6.0);
    ui.c.push_clip(g.inset(0.5));
    // The cycle's beats, Gross Beat's panel ruled in musical time: bars strong, beats medium, and the
    // quarter-beats faint when the cycle is short enough to show them.
    let cycle = cycle_of(ui.m);
    let beats = cycle.round() as usize;
    for b in 1..beats {
        let x = g.x + g.w * b as f32 / beats as f32;
        let col = if b % 4 == 0 { plug::FAINT.with_alpha(140) } else { plug::GRID };
        ui.c.fill_rect(Rect::span(x, x + 1.0, g.y + 3.0, g.bottom() - 3.0), col);
    }
    if beats <= 4 {
        for q in 1..beats * 4 {
            if q % 4 == 0 {
                continue;
            }
            let x = g.x + g.w * q as f32 / (beats * 4) as f32;
            ui.c.fill_rect(Rect::span(x, x + 1.0, g.y + 3.0, g.bottom() - 3.0), plug::GRID);
        }
    }
    // The value guides.
    let lh = ui.c.line_height(MARK);
    if sec == TIME {
        // The offset scale: the top edge is the live sound, the bottom edge two bars back.
        for (off, name) in [(0.0, "live"), (1.0, "1 beat"), (4.0, "1 bar"), (8.0, "2 bars")] {
            let yy = gy(TIME, GridState::y_of_offset(off)).round();
            let strong = off == 0.0;
            ui.c.fill_rect(Rect::span(g.x + 3.0, g.right() - 3.0, yy, yy + 1.0), if strong { plug::DIM } else { plug::FAINT });
            let w = ui.c.text_width(MARK, name);
            // The live line rides the well's top edge: its label sits under it, the rest above their lines.
            let ty = if off == 0.0 { yy + 3.0 } else { yy - lh - 2.0 };
            ui.c.text(MARK, g.right() - 8.0 - w, ty, plug::DIM, name);
        }
        // Gross Beat's angled guides from the top-left corner: the safety line (the pause slope, the offset
        // growing a beat a beat) and, steeper, the reverse guide (two beats a beat).
        let (x0, y0) = (gx(TIME, 0.0), gy(TIME, 0.0));
        let xe = (crate::grid::PANEL_BEATS / cycle).min(1.0);
        let ye = (cycle / crate::grid::PANEL_BEATS).min(1.0);
        ui.c.polyline(&[(x0, y0), (gx(TIME, xe), gy(TIME, ye))], 1.0, plug::DIM.with_alpha(110));
        if cycle <= 4.0 {
            let xr = (crate::grid::PANEL_BEATS / (2.0 * cycle)).min(1.0);
            let yr = (cycle / (crate::grid::PANEL_BEATS / 2.0)).min(1.0);
            ui.c.polyline(&[(x0, y0), (gx(TIME, xr), gy(TIME, yr))], 1.0, plug::FAINT.with_alpha(130));
        }
    } else {
        for (y, name) in [(1.0, "0 dB"), (0.5, "-6"), (0.25, "-12"), (0.125, "-18")] {
            let yy = gy(VOL, y).round();
            let strong = y == 1.0;
            ui.c.fill_rect(Rect::span(g.x + 3.0, g.right() - 3.0, yy, yy + 1.0), if strong { plug::DIM } else { plug::FAINT });
            let w = ui.c.text_width(MARK, name);
            ui.c.text(MARK, g.right() - 8.0 - w, yy + 3.0, plug::DIM, name);
        }
    }
    ui.c.pop_clip();
}

/// The curve: a filled shape down to the grid's floor and the line on top.
fn curve_shape(ui: &mut Ui, sec: usize, curve: &Curve, ink: Color, editing: bool) {
    let g = GRIDS[sec];
    let steps = 120;
    let pts = curve.polyline(steps);
    if pts.is_empty() {
        // An empty section draws its default line (the sound plays it): the live sound on the time grid, the
        // wide-open gate on the volume one.
        let y = if sec == TIME { gy(sec, 0.0) } else { gy(sec, 1.0) };
        ui.c.push_clip(g);
        ui.c.fill_rect(Rect::span(g.x + 2.0, g.right() - 2.0, y.round(), y.round() + 1.0), CURVE_DIM.with_alpha(160));
        ui.c.pop_clip();
        return;
    }
    let xy: Vec<(f32, f32)> = pts.iter().map(|(x, y)| (gx(sec, *x), gy(sec, *y))).collect();
    let alpha_fill = if editing { 46 } else { 30 };
    let alpha_line = if editing { 235 } else { 170 };
    ui.c.push_clip(g);
    let mut fill = xy.clone();
    fill.push((g.right(), g.bottom()));
    fill.push((g.x, g.bottom()));
    ui.c.fill_polygon(&fill, CURVE_FILL.with_alpha(alpha_fill));
    ui.c.polyline(&xy, 1.6, ink.with_alpha(alpha_line));
    ui.c.pop_clip();
}

/// The envelope's points (and the segments' tension handles), the active one bigger.
fn points(ui: &mut Ui, sec: usize, slot: usize, curve: &Curve, active: bool) {
    let col = if active { CURVE_INK } else { CURVE_DIM };
    for (i, p) in curve.pts.iter().enumerate() {
        let (px, py) = (gx(sec, p.x), gy(sec, p.y));
        let big = ui.st.active.is_some_and(|(s, sl, k)| s == sec && sl == slot && k == i);
        let r = if big { 5.0 } else { 4.0 };
        ui.c.fill_circle(px, py, r, POINT_BG);
        ui.c.stroke_circle(px, py, r, if big { 2.0 } else { 1.5 }, col);
        ui.hit(Rect::new(px - 9.0, py - 9.0, 18.0, 18.0), Target::Point(sec, slot, i));
        // The tension handle of the segment this point ends, when it has one and there is room for it.
        if i > 0 {
            let a = &curve.pts[i - 1];
            if Mode::ALL.get(p.m as usize).is_some_and(|m| m.has_tension()) && p.x - a.x > 0.12 {
                let mid = (a.x + p.x) / 2.0;
                let hy = gy(sec, curve.value_at(f64::from(mid)).unwrap_or(0.5));
                let hx = gx(sec, mid);
                ui.c.stroke_circle(hx, hy, 2.5, 1.0, col.with_alpha(190));
                ui.hit(Rect::new(hx - 8.0, hy - 8.0, 16.0, 16.0), Target::Point(sec, slot, i));
            }
        }
    }
}

/// A slot button: the letter in a small choice box, lit when the section plays it.
fn slot_button(ui: &mut Ui, sec: usize, slot: usize) {
    let target = Target::Slot(sec, slot);
    let x = SLOT_LEFT[slot / 2];
    let y = SLOT_TOP + (slot % 2) as f32 * (SLOT_SIZE + 10.0) + sec as f32 * 208.0;
    let r = Rect::new(x, y, SLOT_SIZE, SLOT_SIZE);
    let st = ui.state(r, &target, false);
    let on = ui.slot_of(sec) == slot;
    let ink = lemon::choice_button(ui.c, r, on, brand::MAIN, st);
    ui.c.text_center(LABEL, r, ink, slot_name(slot));
    ui.hit(r, target);
}

/// The cycle picker: a small label and four choice buttons in the band between the wells, the active length lit.
fn cycle_picker(ui: &mut Ui) {
    let names = ["½ BAR", "1 BAR", "2 BARS", "4 BARS"];
    let on = (ui.m.plain("cycle").round() as i32).clamp(0, 3) as usize;
    let gap = 8.0;
    let label_w = 44.0;
    let group_w = label_w + 8.0 + 4.0 * CYCLE_BW + 3.0 * gap;
    let x0 = (GRIDS[0].x + GRIDS[0].right()) / 2.0 - group_w / 2.0;
    let lh = ui.c.line_height(LABEL);
    ui.c.text(LABEL, x0, CYCLE_Y + (CYCLE_H - lh) / 2.0, plug::DIM, "CYCLE");
    for (i, name) in names.iter().enumerate() {
        let r = Rect::new(x0 + label_w + 8.0 + i as f32 * (CYCLE_BW + gap), CYCLE_Y, CYCLE_BW, CYCLE_H);
        let target = Target::Cycle(i);
        let st = ui.state(r, &target, false);
        let ink = lemon::choice_button(ui.c, r, on == i, brand::MAIN, st);
        ui.c.text_center(LABEL, r, ink, name);
        ui.hit(r, target);
    }
}

/// A pith dial of radius `r`, caption under it.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32, cap: &str, bipolar: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0);
    let st = ui.state(hr, &target, false);
    lemon::dial(ui.c, cx, cy, r, 2.8, ui.m.norm(id), bipolar, brand::MAIN, st);
    inst::caption(ui.c, ui.t, cx, cy + r + 8.0, cap, false);
    ui.hit(hr, target);
}

/// The wordmark on the foot's line: "MANGO" in mango beside a large grey "BEAT"; returns its
/// left edge.
fn logo(ui: &mut Ui) -> f32 {
    let wb = lemon::spaced_width(ui.c, MARK_BIG, "BEAT", 2.0);
    let ws = lemon::spaced_width(ui.c, MARK_SMALL, "MANGO", 2.5);
    let (lh_b, lh_s) = (ui.c.line_height(MARK_BIG), ui.c.line_height(MARK_SMALL));
    let bx = (RIGHT - wb).round();
    lemon::spaced(ui.c, MARK_BIG, bx, (FOOT - lh_b / 2.0).round(), plug::WORDMARK, "BEAT", 2.0);
    let sx = (bx - 8.0 - ws).round();
    lemon::spaced(ui.c, MARK_SMALL, sx, (FOOT - lh_s / 2.0).round() + 1.0, brand::MAIN, "MANGO", 2.5);
    let left = sx;
    ui.hit(Rect::span(left - 2.0, RIGHT, FOOT - 12.0, FOOT + 12.0), Target::Logo);
    left
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §3)

/// The hint line for a target; `grids` give the envelope texts.
pub fn hint(t: &Target, m: &dyn Model, grids: &GridState) -> String {
    match t {
        Target::Logo => "Mango Beat".into(),
        Target::Knob(id) => match params_of(id) {
            Some(k) => format!("{}: {}", crate::params::DEFS[k].name, m.text(id)),
            None => format!("{}: {}", id, m.text(id)),
        },
        Target::Slot(sec, slot) => {
            let name = if *sec == TIME { "Time slot" } else { "Volume slot" };
            format!("{}: {}", name, slot_name(*slot))
        }
        Target::Cycle(i) => format!("Cycle: {}", crate::params::value_text(crate::params::CYCLE, *i as i32)),
        Target::Point(sec, slot, i) => point_hint(*sec, *slot, *i, grids, cycle_of(m)),
        Target::Grid(sec) => {
            let name = if *sec == TIME { "Time grid" } else { "Volume grid" };
            format!("{name}: click to plant a point")
        }
    }
}

fn params_of(id: &str) -> Option<usize> {
    crate::params::DEFS.iter().position(|d| d.id == id)
}

/// A point's hint: its place in the cycle and its value on its section's scale.
pub fn point_hint(sec: usize, slot: usize, i: usize, grids: &GridState, cycle: f32) -> String {
    let curve = if sec == TIME { &grids.time[slot.min(3)] } else { &grids.volume[slot.min(3)] };
    let Some(p) = curve.pts.get(i) else { return "Point".into() };
    let at = if p.x <= 0.0 {
        "the start".into()
    } else {
        format!("beat {:.2}", p.x * cycle)
    };
    let name = if sec == TIME { "Time" } else { "Volume" };
    let value = if sec == TIME { offset_text(GridState::offset_of_y(p.y)) } else { db_text(p.y) };
    format!("{name} {}, point {}: {at}, {value}", slot_name(slot), i + 1)
}

/// The settings the dials and slots show.
pub fn values(m: &dyn Model) -> dsp::Settings {
    dsp::Settings {
        time_mix: m.plain("time_mix") / 10000.0,
        vol_mix: m.plain("vol_mix") / 10000.0,
        time_slot: (m.plain("time_slot").round() as i32).clamp(0, 3) as usize,
        vol_slot: (m.plain("vol_slot").round() as i32).clamp(0, 3) as usize,
        cycle_beats: crate::params::CYCLE_BEATS[(m.plain("cycle").round() as i32).clamp(0, 3) as usize],
    }
}
