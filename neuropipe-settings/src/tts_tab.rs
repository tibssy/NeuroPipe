use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use libadwaita::prelude::*;
use serde_json::Value;

use crate::config;
use crate::tts;

const DEFAULT_TEST_SENTENCE: &str =
    "The quick brown fox jumps over the lazy dog. How does my voice sound?";

const EQ_BARS: usize = 4;

/// Shared callback that renders a button/indicator state (and optional detail).
type RenderFn = Rc<dyn Fn(ButtonState, Option<&str>)>;

/// Shared callback that re-renders the per-engine favorite chips.
type RefreshFn = Rc<dyn Fn()>;

/// Builds the TTS tab: engine selector + voice selector, populated from the
/// installed models, persisting changes straight to config.toml.
pub fn build_tts_tab() -> gtk::Widget {
    let page = libadwaita::PreferencesPage::new();

    let group = libadwaita::PreferencesGroup::new();
    group.add_css_class("tts-heading");
    group.set_title("Engine &amp; Voice");
    group.set_description(Some(
        "Which engine renders speech, and which voice to use by default.",
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

    // Favorites: a "+" suffix on the voice row adds the current voice; a
    // 3-column chip list below shows the favorites for the current engine.
    // NOTE: the header must be its own row — ActionRow's set_child replaces the
    // title/subtitle layout, so the FlowBox lives in a separate PreferencesRow.
    let favorites_flow = gtk::FlowBox::new();
    favorites_flow.set_max_children_per_line(3);
    favorites_flow.set_min_children_per_line(3);
    favorites_flow.set_column_spacing(6);
    favorites_flow.set_row_spacing(6);
    favorites_flow.set_halign(gtk::Align::Fill);
    favorites_flow.set_homogeneous(true);
    favorites_flow.set_selection_mode(gtk::SelectionMode::None);

    let favorites_header = libadwaita::ActionRow::new();
    favorites_header.set_title("Favorites");
    favorites_header.set_subtitle(
        "Use + to add the current voice, a chip to make it the default. Starred voices power switching and cycling.",
    );

    let favorites_flow_row = libadwaita::PreferencesRow::new();
    favorites_flow_row.set_child(Some(&favorites_flow));
    favorites_flow_row.set_visible(false);

    let add_fav_button = gtk::Button::new();
    add_fav_button.set_icon_name("list-add-symbolic");
    add_fav_button.add_css_class("flat");
    add_fav_button.set_valign(gtk::Align::Center);
    add_fav_button.set_tooltip_text(Some("Add the currently selected voice to your favorites"));
    favorites_header.add_suffix(&add_fav_button);

    // Indirection so chips (built inside the refresh closure) can trigger
    // another refresh after a removal.
    let favorites_holder: Rc<RefCell<Option<RefreshFn>>> = Rc::new(RefCell::new(None));

    let refresh_favorites: RefreshFn = {
        let shared_engine = Rc::clone(&shared_engine);
        let favorites_flow = favorites_flow.clone();
        let favorites_flow_row = favorites_flow_row.clone();
        let voice_row = voice_row.clone();
        let add_fav_button = add_fav_button.clone();
        let favorites_holder = Rc::clone(&favorites_holder);
        Rc::new(move || {
            let engine = shared_engine.borrow().clone();
            let voices = tts::list_voices(&engine);
            add_fav_button.set_sensitive(!voices.is_empty());
            let favorites = config::tts_favorite_voices(&engine);
            favorites_flow_row.set_visible(!favorites.is_empty());
            favorites_flow.remove_all();
            for voice in &favorites {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                chip.add_css_class("chip");
                chip.set_hexpand(true);
                chip.set_halign(gtk::Align::Fill);

                let select = gtk::Button::new();
                select.add_css_class("flat");
                select.set_hexpand(true);
                select.set_halign(gtk::Align::Fill);
                let select_label = gtk::Label::new(Some(voice));
                select_label.set_xalign(0.0);
                select.set_child(Some(&select_label));
                select.set_tooltip_text(Some("Make this the default voice"));

                let remove = gtk::Button::from_icon_name("window-close-symbolic");
                remove.add_css_class("flat");
                remove.add_css_class("circular");
                remove.set_valign(gtk::Align::Center);
                remove.set_halign(gtk::Align::End);
                remove.set_tooltip_text(Some("Remove from favorites"));

                chip.append(&select);
                chip.append(&remove);
                favorites_flow.append(&chip);

                {
                    let voice = voice.clone();
                    let voices = voices.clone();
                    let voice_row = voice_row.clone();
                    select.connect_clicked(move |_| {
                        if let Some(idx) = voices.iter().position(|v| *v == voice) {
                            voice_row.set_selected(idx as u32);
                        }
                    });
                }
                {
                    let engine = engine.clone();
                    let voice = voice.clone();
                    let favorites_holder = Rc::clone(&favorites_holder);
                    remove.connect_clicked(move |_| {
                        if let Err(error) = config::persist_tts_favorite_remove(&engine, &voice) {
                            eprintln!("[settings] failed to remove favorite: {error}");
                        }
                        if let Some(refresh) = favorites_holder.borrow().as_ref() {
                            refresh();
                        }
                    });
                }
            }
        })
    };
    *favorites_holder.borrow_mut() = Some(Rc::clone(&refresh_favorites));
    refresh_favorites();

    // "+" on the voice row: add the current selection as a favorite.
    {
        let shared_engine = Rc::clone(&shared_engine);
        let shared_voice = Rc::clone(&shared_voice);
        let favorites_holder = Rc::clone(&favorites_holder);
        add_fav_button.connect_clicked(move |_| {
            let engine = shared_engine.borrow().clone();
            let voice = shared_voice.borrow().clone();
            if voice.is_empty() {
                return;
            }
            if config::tts_favorite_voices(&engine).contains(&voice) {
                return;
            }
            if let Err(error) = config::persist_tts_favorite_add(&engine, &voice) {
                eprintln!("[settings] failed to add favorite: {error}");
                return;
            }
            if let Some(refresh) = favorites_holder.borrow().as_ref() {
                refresh();
            }
        });
    }

    suppress_persist.set(true);
    populate_voice_row(&voice_row, &current_engine, &current_voice);
    populate_speed_row(&speed_row, &current_engine);
    populate_quality_row(&quality_row, &current_engine);
    timeout_row.set_value(config::tts_idle_timeout_sec() as f64);
    suppress_persist.set(false);

    group.add(&engine_row);
    group.add(&voice_row);
    group.add(&favorites_header);
    group.add(&favorites_flow_row);
    group.add(&speed_row);
    group.add(&quality_row);
    page.add(&group);

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
        let favorites_holder = Rc::clone(&favorites_holder);
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
            if let Some(refresh) = favorites_holder.borrow().as_ref() {
                refresh();
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

    page.add(&build_test_group(
        Rc::clone(&shared_engine),
        Rc::clone(&shared_voice),
        &speed_row,
        &quality_row,
    ));

    // Idle timeout at the end, just below the test voice card.
    let timeout_group = libadwaita::PreferencesGroup::new();
    timeout_group.add(&timeout_row);
    page.add(&timeout_group);

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

/// Builds the "Test Voice" group: an editable sentence and a stateful play
/// button whose icon reflects the live TTS state — play = idle, spinner =
/// processing/generating, animated wave = speaking.
fn build_test_group(
    shared_engine: Rc<RefCell<String>>,
    shared_voice: Rc<RefCell<String>>,
    speed_row: &libadwaita::SpinRow,
    quality_row: &libadwaita::ComboRow,
) -> libadwaita::PreferencesGroup {
    let group = libadwaita::PreferencesGroup::new();

    let sentence_entry = gtk::Entry::new();
    sentence_entry.set_text(DEFAULT_TEST_SENTENCE);
    sentence_entry.set_placeholder_text(Some("Text to speak"));
    sentence_entry.set_tooltip_text(Some("Text spoken when you press Play test"));

    let play_icon = gtk::Image::from_icon_name("media-playback-start");

    let spinner = gtk::Spinner::new();

    let wave = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    wave.set_valign(gtk::Align::Center);
    wave.set_halign(gtk::Align::Center);
    for i in 0..EQ_BARS {
        let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bar.add_css_class("eq-bar");
        bar.add_css_class(&format!("eq-bar-{i}"));
        bar.set_valign(gtk::Align::Center);
        bar.set_halign(gtk::Align::Center);
        wave.append(&bar);
    }

    let play_button = gtk::Button::new();
    play_button.add_css_class("suggested-action");
    play_button.set_halign(gtk::Align::Center);
    play_button.set_valign(gtk::Align::Center);
    play_button.set_size_request(36, 36);
    play_button.set_child(Some(&play_icon));

    let status_label = gtk::Label::new(Some("Idle"));
    status_label.set_css_classes(&["status-idle"]);
    status_label.set_wrap(true);
    status_label.set_max_width_chars(48);

    // Header row inside the card: title + subtitle on the left, state
    // indicator (text) and the play button at the end of the row.
    let header = libadwaita::ActionRow::new();
    header.set_title("Test Voice");
    header.set_subtitle("Speak a sample sentence with the current settings.");
    let header_suffix = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header_suffix.set_valign(gtk::Align::Center);
    status_label.set_valign(gtk::Align::Center);
    header_suffix.append(&status_label);
    header_suffix.append(&play_button);
    header.add_suffix(&header_suffix);
    group.add(&header);

    // Text input in the same card, below the header row.
    let row = libadwaita::PreferencesRow::new();
    row.set_child(Some(&sentence_entry));
    group.add(&row);

    let state = Rc::new(RefCell::new(ButtonState::Idle));
    let processing_since = Rc::new(RefCell::new(None));

    let render: RenderFn = {
        let play_button = play_button.clone();
        let spinner = spinner.clone();
        let wave = wave.clone();
        let play_icon = play_icon.clone();
        let status_label = status_label.clone();
        let state = Rc::clone(&state);
        let processing_since = Rc::clone(&processing_since);
        Rc::new(move |next, detail| {
            if next != ButtonState::Processing {
                *processing_since.borrow_mut() = None;
            } else if processing_since.borrow().is_none() {
                *processing_since.borrow_mut() = Some(Instant::now());
            }
            *state.borrow_mut() = next;
            spinner.stop();
            match next {
                ButtonState::Idle => {
                    play_button.set_child(Some(&play_icon));
                    wave.remove_css_class("eq-active");
                    status_label.set_text("Idle");
                    status_label.set_css_classes(&["status-idle"]);
                }
                ButtonState::Processing => {
                    play_button.set_child(Some(&spinner));
                    spinner.start();
                    status_label.set_text("Processing…");
                    status_label.set_css_classes(&["status-idle"]);
                }
                ButtonState::Speaking => {
                    play_button.set_child(Some(&wave));
                    wave.add_css_class("eq-active");
                    status_label.set_text("Speaking");
                    status_label.set_css_classes(&["status-active"]);
                }
                ButtonState::Unavailable => {
                    play_button.set_child(Some(&play_icon));
                    wave.remove_css_class("eq-active");
                    status_label.set_text(detail.unwrap_or("Service unavailable"));
                    status_label.set_css_classes(&["status-inactive"]);
                }
            }
        })
    };

    let run_test: Rc<dyn Fn()> = {
        let shared_engine = Rc::clone(&shared_engine);
        let shared_voice = Rc::clone(&shared_voice);
        let speed_row = speed_row.clone();
        let quality_row = quality_row.clone();
        let sentence_entry = sentence_entry.clone();
        let render = Rc::clone(&render);
        Rc::new(move || {
            let text = sentence_entry.text().to_string();
            if text.trim().is_empty() {
                render(ButtonState::Unavailable, Some("Enter some text to speak"));
                return;
            }
            let engine = shared_engine.borrow().clone();
            let voice = shared_voice.borrow().clone();
            let speed = speed_row.value();
            let quality =
                selected_string(&quality_row).unwrap_or_else(|| config::tts_quality_for(&engine));
            render(ButtonState::Processing, None);
            match crate::ipc::tts_speak(&text, &engine, &voice, speed, &quality) {
                Ok(reply) => {
                    if reply.get("status").and_then(Value::as_str) != Some("queued") {
                        let message = reply
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error");
                        render(ButtonState::Unavailable, Some(message));
                    }
                    // queued: hold the spinner until the poller reports speaking
                }
                Err(error) => render(ButtonState::Unavailable, Some(&error)),
            }
        })
    };
    {
        let run_test = Rc::clone(&run_test);
        play_button.connect_clicked(move |_| run_test());
    }
    {
        let run_test = Rc::clone(&run_test);
        sentence_entry.connect_activate(move |_| run_test());
    }

    // Live state: a background thread polls the service; updates are marshaled
    // back to the GTK main thread via an idle source.
    let (tx, rx) = mpsc::channel();
    {
        let render = Rc::clone(&render);
        let state = Rc::clone(&state);
        let processing_since = Rc::clone(&processing_since);
        glib::idle_add_local(move || {
            while let Ok(observed) = rx.try_recv() {
                let current = *state.borrow();
                let next = match observed {
                    None => ButtonState::Unavailable,
                    Some(true) => ButtonState::Speaking,
                    Some(false) => match current {
                        // Generating a test sentence: hold the spinner until
                        // playback actually starts, unless it has been stuck
                        // for a while (e.g. generation silently failed).
                        ButtonState::Processing => {
                            let stuck = processing_since
                                .borrow()
                                .map(|t| t.elapsed() >= Duration::from_secs(8))
                                .unwrap_or(false);
                            if stuck {
                                ButtonState::Idle
                            } else {
                                ButtonState::Processing
                            }
                        }
                        // Just finished playing.
                        ButtonState::Speaking => ButtonState::Idle,
                        // Reachable and quiet (idle, or recovered from a down service).
                        _ => ButtonState::Idle,
                    },
                };
                if next != current {
                    render(next, None);
                }
            }
            glib::ControlFlow::Continue
        });
    }
    crate::ipc::spawn_speaking_poller(tx);

    group
}

/// Button/indicator lifecycle states for the test voice control.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonState {
    Idle,
    Processing,
    Speaking,
    Unavailable,
}
