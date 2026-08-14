use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita::prelude::*;

use crate::config;
use crate::tts;

/// Builds the TTS tab: engine selector + voice selector, populated from the
/// installed models, persisting changes straight to config.toml.
pub fn build_tts_tab() -> gtk::Widget {
    let page = libadwaita::PreferencesPage::new();

    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Engine &amp; Voice");
    group.set_description(Some(
        "Which engine the TTS service uses, and which voice to speak in.",
    ));

    let (current_engine, current_voice) = config::tts_defaults();

    let engine_row = libadwaita::ComboRow::new();
    engine_row.set_title("Engine");
    engine_row.set_subtitle("Text-to-speech backend used to synthesize speech");
    let engine_model = gtk::StringList::new(&tts::ENGINES);
    engine_row.set_model(Some(&engine_model));
    let engine_idx = tts::ENGINES
        .iter()
        .position(|e| *e == current_engine)
        .unwrap_or(0);
    engine_row.set_selected(engine_idx as u32);

    let voice_row = libadwaita::ComboRow::new();
    voice_row.set_title("Voice");
    voice_row.set_subtitle("Default voice for the selected engine");
    let speed_row = libadwaita::SpinRow::with_range(0.5, 2.0, 0.1);
    speed_row.set_title("Speed");
    speed_row.set_subtitle("How fast speech is played back (0.5–2.0x)");
    speed_row.set_digits(1);

    let quality_row = libadwaita::ComboRow::new();
    quality_row.set_title("Quality");
    quality_row.set_subtitle("Per-engine model quality: high uses the full model, low is faster");

    let timeout_row = libadwaita::SpinRow::with_range(1.0, 600.0, 5.0);
    timeout_row.set_title("Idle timeout");
    timeout_row.set_subtitle("Seconds of inactivity before the model is unloaded");
    timeout_row.set_digits(0);

    let shared_engine = Rc::new(RefCell::new(current_engine.clone()));
    let shared_voice = Rc::new(RefCell::new(current_voice.clone()));
    let suppress_persist = Rc::new(Cell::new(false));

    suppress_persist.set(true);
    populate_voice_row(&voice_row, &current_engine, &current_voice);
    populate_speed_row(&speed_row, &current_engine);
    populate_quality_row(&quality_row, &current_engine);
    timeout_row.set_value(config::tts_idle_timeout_sec() as f64);
    suppress_persist.set(false);

    group.add(&engine_row);
    group.add(&voice_row);
    group.add(&speed_row);
    group.add(&quality_row);
    group.add(&timeout_row);
    page.add(&group);

    let hint_group = libadwaita::PreferencesGroup::new();
    hint_group.add(&apply_hint());
    page.add(&hint_group);

    // Engine changed: rebuild the voice list and refresh the per-engine speed
    // and quality, keeping the current voice when it still exists for the new
    // engine, otherwise falling back to the first.
    {
        let engine_row = engine_row.clone();
        let voice_row = voice_row.clone();
        let speed_row = speed_row.clone();
        let quality_row = quality_row.clone();
        let shared_engine = Rc::clone(&shared_engine);
        let shared_voice = Rc::clone(&shared_voice);
        let suppress_persist = Rc::clone(&suppress_persist);
        engine_row.clone().connect_selected_notify(move |_| {
            let Some(engine) = selected_string(&engine_row) else {
                return;
            };
            *shared_engine.borrow_mut() = engine.clone();
            let voices = tts::list_voices(&engine);
            let voice = voices
                .iter()
                .find(|v| **v == *shared_voice.borrow())
                .cloned()
                .or_else(|| voices.first().cloned())
                .unwrap_or_default();
            suppress_persist.set(true);
            populate_voice_row(&voice_row, &engine, &voice);
            populate_speed_row(&speed_row, &engine);
            populate_quality_row(&quality_row, &engine);
            suppress_persist.set(false);
            *shared_voice.borrow_mut() = voice.clone();
            if let Err(error) = config::persist_tts_defaults(&engine, &voice) {
                eprintln!("[settings] failed to persist TTS defaults: {error}");
            }
            crate::ipc::notify_tts_reload();
        });
    }

    // Voice changed: persist engine + voice to config.
    {
        let voice_row = voice_row.clone();
        let shared_engine = Rc::clone(&shared_engine);
        let shared_voice = Rc::clone(&shared_voice);
        let suppress_persist = Rc::clone(&suppress_persist);
        voice_row.clone().connect_selected_notify(move |_| {
            if suppress_persist.get() {
                return;
            }
            let Some(voice) = selected_string(&voice_row) else {
                return;
            };
            let engine = shared_engine.borrow().clone();
            *shared_voice.borrow_mut() = voice.clone();
            if let Err(error) = config::persist_tts_defaults(&engine, &voice) {
                eprintln!("[settings] failed to persist TTS defaults: {error}");
            }
            crate::ipc::notify_tts_reload();
        });
    }

    // Speed changed: persist the per-engine override.
    {
        let speed_row = speed_row.clone();
        let shared_engine = Rc::clone(&shared_engine);
        let suppress_persist = Rc::clone(&suppress_persist);
        speed_row.clone().connect_value_notify(move |_| {
            if suppress_persist.get() {
                return;
            }
            let engine = shared_engine.borrow().clone();
            if let Err(error) = config::persist_tts_speed(&engine, speed_row.value()) {
                eprintln!("[settings] failed to persist TTS speed: {error}");
            }
            crate::ipc::notify_tts_reload();
        });
    }

    // Quality changed: persist the per-engine override.
    {
        let quality_row = quality_row.clone();
        let shared_engine = Rc::clone(&shared_engine);
        let suppress_persist = Rc::clone(&suppress_persist);
        quality_row.clone().connect_selected_notify(move |_| {
            if suppress_persist.get() {
                return;
            }
            let Some(quality) = selected_string(&quality_row) else {
                return;
            };
            let engine = shared_engine.borrow().clone();
            if let Err(error) = config::persist_tts_quality(&engine, &quality) {
                eprintln!("[settings] failed to persist TTS quality: {error}");
            }
            crate::ipc::notify_tts_reload();
        });
    }

    // Idle timeout changed: persist the global default.
    {
        let timeout_row = timeout_row.clone();
        let suppress_persist = Rc::clone(&suppress_persist);
        timeout_row.clone().connect_value_notify(move |_| {
            if suppress_persist.get() {
                return;
            }
            let secs = timeout_row.value() as u64;
            if let Err(error) = config::persist_tts_idle_timeout(secs) {
                eprintln!("[settings] failed to persist TTS idle timeout: {error}");
            }
            crate::ipc::notify_tts_reload();
        });
    }

    page.upcast()
}

/// Fills the voice ComboRow for `engine` and selects `voice` when available.
fn populate_voice_row(row: &libadwaita::ComboRow, engine: &str, voice: &str) {
    let voices = tts::list_voices(engine);
    let voice_model = gtk::StringList::new(&voices.iter().map(String::as_str).collect::<Vec<_>>());
    row.set_model(Some(&voice_model));

    if voices.is_empty() {
        row.set_subtitle("Default voice for the selected engine — none installed yet");
        return;
    }
    row.set_subtitle("Default voice for the selected engine");
    let idx = voices.iter().position(|v| v == voice).unwrap_or(0);
    row.set_selected(idx as u32);
}

fn selected_string(row: &libadwaita::ComboRow) -> Option<String> {
    let model = row.model()?.downcast::<gtk::StringList>().ok()?;
    let idx = row.selected();
    model.string(idx).map(|s| s.to_string())
}

/// Sets the speed row to the effective speed for `engine` (per-engine override
/// if present, else the global default).
fn populate_speed_row(row: &libadwaita::SpinRow, engine: &str) {
    row.set_value(config::tts_speed_for(engine));
}

/// Fills the quality ComboRow with the effective quality for `engine`.
fn populate_quality_row(row: &libadwaita::ComboRow, engine: &str) {
    let qualities = ["low", "high"];
    let model = gtk::StringList::new(&qualities);
    row.set_model(Some(&model));
    let current = config::tts_quality_for(engine);
    let idx = qualities.iter().position(|q| *q == current).unwrap_or(1);
    row.set_selected(idx as u32);
}

fn apply_hint() -> gtk::Label {
    let hint = gtk::Label::new(Some(
        "Changes are saved to config.toml and take effect the next time the TTS service loads it.",
    ));
    hint.add_css_class("dim-label");
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.set_margin_top(4);
    hint.set_margin_start(16);
    hint.set_margin_end(16);
    hint
}
