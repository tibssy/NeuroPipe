use std::rc::Rc;

use gtk::prelude::*;
use libadwaita::prelude::*;

use crate::config;

/// Persist one `[stt]` scalar and ask the live service to re-read config.toml.
fn persist_stt(key: &str, value: toml::Value) {
    if let Err(error) = config::persist_stt_scalar(key, value) {
        eprintln!("[settings] failed to persist STT {key}: {error}");
        return;
    }
    crate::ipc::notify_stt_reload();
}

/// Persist a bundled model preset's id and directory together, then reload.
fn persist_stt_model(model: &str, model_dir: &str) {
    if let Err(error) = config::persist_stt_model(model, model_dir) {
        eprintln!("[settings] failed to persist STT model: {error}");
        return;
    }
    crate::ipc::notify_stt_reload();
}

fn stt_int(value: f64) -> toml::Value {
    toml::Value::Integer(value as i64)
}

fn stt_float(value: f64) -> toml::Value {
    toml::Value::Float(value)
}

/// Config string with the service default when the key is unset.
fn stt_string_or(key: &str, fallback: &str) -> String {
    let value = config::stt_string(key);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

/// An editable titled row: an entry lives in the row's suffix, exactly like the
/// services' config fields it edits. Enter commits the new value.
fn entry_row(title: &str, subtitle: &str, initial: &str) -> (libadwaita::ActionRow, gtk::Entry) {
    let row = libadwaita::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_hexpand(true);
    entry.set_halign(gtk::Align::End);
    entry.set_valign(gtk::Align::Center);
    row.add_suffix(&entry);
    row.set_activatable_widget(Some(&entry));
    (row, entry)
}

/// Wrapped SpinRow with the given range, digits and initial value.
fn spin_row(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    digits: u32,
    initial: f64,
) -> libadwaita::SpinRow {
    let row = libadwaita::SpinRow::with_range(min, max, step);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_digits(digits);
    row.set_value(initial.clamp(min, max));
    row
}

/// Builds the STT tab: ASR model, VAD/capture tuning and turn-end detection,
/// persisting every change straight to config.toml and hot-applying it via the
/// service's reload_config command.
pub fn build_stt_tab() -> gtk::Widget {
    let page = libadwaita::PreferencesPage::new();

    // ---- ASR Model -----------------------------------------------------
    let model_group = libadwaita::PreferencesGroup::new();
    model_group.set_title("ASR Model");
    model_group.set_description(Some(
        "Speech recognition backend used to transcribe the microphone. Model switches apply on the next transcription, without a service restart.",
    ));

    let model_row = libadwaita::ComboRow::new();
    model_row.set_title("Model");
    model_row.set_subtitle("Bundled ASR model; also selects its model directory");
    let labels = config::STT_MODEL_PRESETS
        .iter()
        .map(|(label, _, _)| *label)
        .collect::<Vec<_>>();
    model_row.set_model(Some(&gtk::StringList::new(&labels)));

    let model_dir_initial = stt_string_or("model_dir", config::DEFAULT_STT_MODEL_DIR);
    let model_initial = stt_string_or("model", config::DEFAULT_STT_MODEL);
    let selected = config::STT_MODEL_PRESETS
        .iter()
        .position(|(_, _, dir)| *dir == model_dir_initial)
        .or_else(|| {
            config::STT_MODEL_PRESETS
                .iter()
                .position(|(_, model, _)| *model == model_initial)
        })
        .unwrap_or(0);
    model_row.set_selected(selected as u32);

    let (model_dir_row, model_dir_entry) = entry_row(
        "Model directory",
        "Directory containing the recognizer models (~ is expanded)",
        &model_dir_initial,
    );
    let idle_timeout_row = spin_row(
        "Model idle timeout",
        "Seconds of inactivity before the recognition model is unloaded from memory",
        1.0,
        3600.0,
        10.0,
        0,
        config::stt_int("model_idle_timeout_sec", config::DEFAULT_STT_MODEL_IDLE_TIMEOUT_SEC) as f64,
    );

    model_group.add(&model_row);
    model_group.add(&model_dir_row);
    model_group.add(&idle_timeout_row);
    page.add(&model_group);

    model_row.connect_selected_notify({
        let model_dir_entry = model_dir_entry.clone();
        move |row| {
            let Some(preset) = config::STT_MODEL_PRESETS.get(row.selected() as usize) else {
                return;
            };
            let (_, model, dir) = *preset;
            model_dir_entry.set_text(dir);
            persist_stt_model(model, dir);
        }
    });
    model_dir_entry.connect_activate({
        let model_dir_entry = model_dir_entry.clone();
        move |_| {
            let value = toml::Value::String(model_dir_entry.text().trim().to_string());
            persist_stt("model_dir", value);
        }
    });

    // ---- Voice Activity (VAD) ------------------------------------------
    let vad_group = libadwaita::PreferencesGroup::new();
    vad_group.set_title("Voice Activity");
    vad_group.set_description(Some(
        "How the microphone decides when speech starts, how loudly it is captured, and how long it keeps listening.",
    ));

    let vad_threshold_row = spin_row(
        "VAD threshold",
        "Speech confidence above which audio counts as speech (0–1)",
        0.0,
        1.0,
        0.01,
        2,
        config::stt_float("vad_threshold", config::DEFAULT_STT_VAD_THRESHOLD),
    );
    let vad_start_frames_row = spin_row(
        "Start frames",
        "Consecutive speech frames required to begin recording (debounces noise)",
        1.0,
        200.0,
        1.0,
        0,
        config::stt_int("vad_start_frames", config::DEFAULT_STT_VAD_START_FRAMES) as f64,
    );
    let gain_row = spin_row(
        "Digital gain",
        "Amplification applied to the microphone before VAD and recognition",
        0.1,
        10.0,
        0.1,
        1,
        config::stt_float("digital_gain", config::DEFAULT_STT_DIGITAL_GAIN),
    );

    vad_group.add(&vad_threshold_row);
    vad_group.add(&vad_start_frames_row);
    vad_group.add(&gain_row);
    page.add(&vad_group);

    vad_threshold_row.connect_value_notify({
        let row = vad_threshold_row.clone();
        move |_| {
            persist_stt("vad_threshold", stt_float(row.value()));
        }
    });
    vad_start_frames_row.connect_value_notify({
        let row = vad_start_frames_row.clone();
        move |_| {
            persist_stt("vad_start_frames", stt_int(row.value()));
        }
    });
    gain_row.connect_value_notify({
        let row = gain_row.clone();
        move |_| {
            persist_stt("digital_gain", stt_float(row.value()));
        }
    });
    idle_timeout_row.connect_value_notify({
        let row = idle_timeout_row.clone();
        move |_| {
            persist_stt("model_idle_timeout_sec", stt_int(row.value()));
        }
    });

    // ---- Turn-end detection ---------------------------------------------
    let turn_group = libadwaita::PreferencesGroup::new();
    turn_group.set_title("Turn-End Detection");
    turn_group.set_description(Some(
        "When to stop recording and finalize a transcript. Disable to end turns on a fixed silence timeout instead.",
    ));

    let turn_enabled_row = libadwaita::SwitchRow::new();
    turn_enabled_row.set_title("Turn-end detection");
    turn_enabled_row.set_subtitle(
        "End turns based on speech prosody rather than a fixed silence timeout",
    );
    turn_enabled_row.set_active(config::stt_bool("turn_end_enabled", config::DEFAULT_STT_TURN_END_ENABLED));

    let silence_timeout_row = spin_row(
        "Silence timeout (s)",
        "Seconds of quiet that ends a turn when turn-end detection is off",
        0.1,
        30.0,
        0.1,
        1,
        config::stt_float("silence_timeout_sec", config::DEFAULT_STT_SILENCE_TIMEOUT_SEC),
    );

    let detector_row = libadwaita::ComboRow::new();
    detector_row.set_title("Detector");
    detector_row.set_subtitle("smart_turn uses the ONNX classifier, heuristic uses a prosodic scorer");
    let detector_options = ["smart_turn", "heuristic"];
    let detector_model = gtk::StringList::new(&detector_options);
    detector_row.set_model(Some(&detector_model));
    let current_detector = stt_string_or("turn_detector", config::DEFAULT_STT_TURN_DETECTOR);
    let detector_idx = detector_options
        .iter()
        .position(|d| *d == current_detector)
        .unwrap_or(0);
    detector_row.set_selected(detector_idx as u32);

    let hold_ms_row = spin_row(
        "Turn hold (ms)",
        "Silence a speaker must hold before the detector is consulted",
        0.0,
        5000.0,
        50.0,
        0,
        config::stt_int("turn_hold_ms", config::DEFAULT_STT_TURN_HOLD_MS) as f64,
    );
    let threshold_row = spin_row(
        "End threshold",
        "P(end-of-turn) above which the turn is finalized early (0–1)",
        0.0,
        1.0,
        0.01,
        2,
        config::stt_float("turn_end_threshold", config::DEFAULT_STT_TURN_END_THRESHOLD),
    );
    let cadence_row = spin_row(
        "Re-score cadence (ms)",
        "How often the turn is re-scored while the speaker keeps pausing",
        100.0,
        2000.0,
        100.0,
        0,
        config::stt_int("turn_score_cadence_ms", config::DEFAULT_STT_TURN_SCORE_CADENCE_MS) as f64,
    );
    let ceiling_row = spin_row(
        "Hard ceiling (ms)",
        "Absolute silence limit; the turn always ends at this point",
        500.0,
        60000.0,
        250.0,
        0,
        config::stt_int("turn_hard_ceiling_ms", config::DEFAULT_STT_TURN_HARD_CEILING_MS) as f64,
    );
    let (smart_path_row, smart_path_entry) = entry_row(
        "Smart-turn model path",
        "Path to the ONNX classifier, stored beside the VAD model (~ is expanded)",
        &stt_string_or("smart_turn_model_path", config::DEFAULT_STT_SMART_TURN_MODEL_PATH),
    );
    let smart_min_utterance_row = spin_row(
        "Min utterance (ms)",
        "Shortest recording the smart-turn model treats as a complete turn",
        100.0,
        10000.0,
        100.0,
        0,
        config::stt_int("smart_turn_min_utterance_ms", config::DEFAULT_STT_SMART_TURN_MIN_UTTERANCE_MS) as f64,
    );

    turn_group.add(&turn_enabled_row);
    turn_group.add(&silence_timeout_row);
    turn_group.add(&detector_row);
    turn_group.add(&hold_ms_row);
    turn_group.add(&threshold_row);
    turn_group.add(&cadence_row);
    turn_group.add(&ceiling_row);
    turn_group.add(&smart_path_row);
    turn_group.add(&smart_min_utterance_row);
    page.add(&turn_group);

    // Sensitivity: the silence timeout is used only when turn-end is disabled;
    // the detector knobs only when it is enabled, and the smart-turn specific
    // rows only when the smart_turn detector is selected.
    fn selected_detector(row: &libadwaita::ComboRow) -> Option<String> {
        let model = row.model()?.downcast::<gtk::StringList>().ok()?;
        let idx = row.selected();
        model.string(idx).map(|s| s.to_string())
    }
    let update_sensitivity: Rc<dyn Fn()> = Rc::new({
        let turn_enabled_row = turn_enabled_row.clone();
        let silence_timeout_row = silence_timeout_row.clone();
        let detector_row = detector_row.clone();
        let hold_ms_row = hold_ms_row.clone();
        let threshold_row = threshold_row.clone();
        let cadence_row = cadence_row.clone();
        let ceiling_row = ceiling_row.clone();
        let smart_path_row = smart_path_row.clone();
        let smart_min_utterance_row = smart_min_utterance_row.clone();
        move || {
            let enabled = turn_enabled_row.is_active();
            let smart = selected_detector(&detector_row).as_deref() == Some("smart_turn");
            silence_timeout_row.set_sensitive(!enabled);
            detector_row.set_sensitive(enabled);
            hold_ms_row.set_sensitive(enabled);
            threshold_row.set_sensitive(enabled);
            cadence_row.set_sensitive(enabled);
            ceiling_row.set_sensitive(enabled);
            smart_path_row.set_sensitive(enabled && smart);
            smart_min_utterance_row.set_sensitive(enabled && smart);
        }
    });
    update_sensitivity();

    smart_path_entry.connect_activate({
        let smart_path_entry = smart_path_entry.clone();
        move |_| {
            persist_stt("smart_turn_model_path", toml::Value::String(smart_path_entry.text().trim().to_string()));
        }
    });

    turn_enabled_row.connect_active_notify({
        let turn_enabled_row = turn_enabled_row.clone();
        let update_sensitivity = Rc::clone(&update_sensitivity);
        move |_| {
            update_sensitivity();
            persist_stt("turn_end_enabled", toml::Value::Boolean(turn_enabled_row.is_active()));
        }
    });
    silence_timeout_row.connect_value_notify({
        let row = silence_timeout_row.clone();
        move |_| {
            persist_stt("silence_timeout_sec", stt_float(row.value()));
        }
    });
    detector_row.connect_selected_notify({
        let detector_row = detector_row.clone();
        let update_sensitivity = Rc::clone(&update_sensitivity);
        move |_| {
            update_sensitivity();
            if let Some(detector) = selected_detector(&detector_row) {
                persist_stt("turn_detector", toml::Value::String(detector.clone()));
            }
        }
    });
    hold_ms_row.connect_value_notify({
        let row = hold_ms_row.clone();
        move |_| {
            persist_stt("turn_hold_ms", stt_int(row.value()));
        }
    });
    threshold_row.connect_value_notify({
        let row = threshold_row.clone();
        move |_| {
            persist_stt("turn_end_threshold", stt_float(row.value()));
        }
    });
    cadence_row.connect_value_notify({
        let row = cadence_row.clone();
        move |_| {
            persist_stt("turn_score_cadence_ms", stt_int(row.value()));
        }
    });
    ceiling_row.connect_value_notify({
        let row = ceiling_row.clone();
        move |_| {
            persist_stt("turn_hard_ceiling_ms", stt_int(row.value()));
        }
    });
    smart_min_utterance_row.connect_value_notify({
        let row = smart_min_utterance_row.clone();
        move |_| {
            persist_stt("smart_turn_min_utterance_ms", stt_int(row.value()));
        }
    });

    page.upcast()
}