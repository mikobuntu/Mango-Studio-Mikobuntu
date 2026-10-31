// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The startup sound: the `startup.wav` baked into the binary, played once when startup is done — the splash has
//! gone (or there was none) and the main window has shown its first frame. `main_body` calls `play_once` every
//! tick; the first one past that point plays, the rest do nothing. The settings' Silent startup mutes it
//! (spec: ui/settings-choices.md).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::{self, App};
use crate::prefs::{check, SILENT_STARTUP};

/// The startup sound, baked into the binary at build time (`assets/samples/startup.wav`).
pub static STARTUP_WAV: &[u8] = include_bytes!("../assets/samples/startup.wav");

/// The sample path the preview project plays: no file of this name exists where the app runs, so the engine
/// decodes the registered bytes instead (os-engine `sample::register_embedded`; a real file of the same name
/// would always win).
const SAMPLE_PATH: &str = "startup.wav";

static REGISTERED: AtomicBool = AtomicBool::new(false);
static PLAYED: AtomicBool = AtomicBool::new(false);

/// The registry entry: the path the preview plays and the bytes behind it (the engine keeps the reference, so it
/// must be `'static`).
static EMBEDDED: &[(&str, &[u8])] = &[(SAMPLE_PATH, STARTUP_WAV)];

/// Points the engine's sample registry at the baked-in bytes: once per process, before the first preview.
fn register() {
    if REGISTERED.swap(true, Ordering::Relaxed) {
        return;
    }
    os_engine::sample::register_embedded(EMBEDDED);
}

/// Plays the startup sound once, as a one-note preview project whose channel holds the baked-in sample (the same
/// path the browser's sample preview plays a file by, spec: ui/browser.md §7). A second call does nothing, and
/// Silent startup mutes it.
pub fn play_once(app: &App) {
    if check(&SILENT_STARTUP) || PLAYED.swap(true, Ordering::Relaxed) {
        return;
    }
    register();
    app.transport.preview_for(app::preview_project(SAMPLE_PATH), None);
}
