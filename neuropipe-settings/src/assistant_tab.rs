use std::cell::RefCell;
use std::rc::Rc;

use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;
use libadwaita::prelude::*;
use toml::Value;

use crate::{config, ipc};

type RefreshFn = Rc<dyn Fn()>;

const DEFAULT_EMBEDDING_MODEL: &str = "all-minilm";

/// Builds all user-facing Assistant settings. Values are saved directly to
/// config.toml; model/tool changes reload live, while the remaining service
/// configuration changes are applied after restarting the Assistant service.
pub fn build_assistant_tab() -> gtk::Widget {
    let page = libadwaita::PreferencesPage::new();
    let (status_row, status_label) = status_row();
    let (chat_models, decision_models) = ipc::assistant_model_lists().unwrap_or_else(|error| {
        eprintln!("[settings] could not query Ollama model capabilities: {error}");
        (Vec::new(), Vec::new())
    });

    build_model_group(&page, &status_row, &status_label, chat_models);
    build_decision_model_group(&page, &status_row, &status_label, decision_models);
    build_test_group(&page);
    build_tools_group(&page, &status_row, &status_label);
    build_memory_group(&page, &status_row, &status_label);
    build_instructions_group(&page, &status_row, &status_label);
    build_search_folders_group(&page, &status_row, &status_label);
    build_ducking_group(&page, &status_row, &status_label);
    build_general_group(&page, &status_row, &status_label);

    let status_group = libadwaita::PreferencesGroup::new();
    status_group.add(&status_row);
    page.add(&status_group);
    page.upcast()
}

fn status_row() -> (libadwaita::PreferencesRow, gtk::Label) {
    let label = gtk::Label::new(None);
    label.add_css_class("dim-label");
    label.set_wrap(true);
    label.set_xalign(0.0);
    let row = libadwaita::PreferencesRow::new();
    row.set_child(Some(&label));
    row.set_visible(false);
    (row, label)
}

fn set_status(row: &libadwaita::PreferencesRow, label: &gtk::Label, text: &str) {
    label.set_text(text);
    row.set_visible(true);
}

fn set_persist_status(
    row: &libadwaita::PreferencesRow,
    label: &gtk::Label,
    result: Result<(), String>,
    apply: ApplyMode,
) {
    match result {
        Ok(()) => match apply {
            ApplyMode::Live => match ipc::notify_assistant_reload() {
                Ok(()) => set_status(row, label, "Saved and applied to the running Assistant."),
                Err(_) => set_status(
                    row,
                    label,
                    "Saved. The Assistant service is not running; this will apply when it starts.",
                ),
            },
            ApplyMode::SearchFolders => set_status(
                row,
                label,
                "Saved. File search uses these folders on its next run.",
            ),
            ApplyMode::Restart => set_status(
                row,
                label,
                "Saved. Restart the Assistant service to apply this setting.",
            ),
            ApplyMode::Placeholder => set_status(
                row,
                label,
                "Saved as a preference only; decision-model routing is not connected yet.",
            ),
            ApplyMode::Favorite => set_status(row, label, "Favorite models saved."),
        },
        Err(error) => {
            eprintln!("[settings] failed to save Assistant setting: {error}");
            set_status(row, label, &format!("Could not save setting: {error}"));
        }
    }
}

#[derive(Clone, Copy)]
enum ApplyMode {
    Live,
    SearchFolders,
    Restart,
    Placeholder,
    Favorite,
}

fn build_model_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
    mut models: Vec<String>,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Chat model");
    group.set_description(Some(
        "Select the default Ollama model. Favorite models are used by model cycling.",
    ));

    let default_model = config::assistant_default_model();
    models.sort();
    models.dedup();

    let model_row = libadwaita::ComboRow::new();
    model_row.set_title("Default model");
    model_row.set_subtitle(if models.is_empty() {
        "No chat-capable Ollama models found"
    } else if models
        .iter()
        .any(|model| model_name_matches(&default_model, model))
    {
        "Chat/completion models only"
    } else {
        "Configured model is unavailable or not chat-capable; choose a listed chat model"
    });
    let model_strings: Vec<&str> = models.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&model_strings);
    model_row.set_model(Some(&model));
    if !models.is_empty() {
        model_row.set_selected(
            models
                .iter()
                .position(|candidate| model_name_matches(&default_model, candidate))
                .unwrap_or(0) as u32,
        );
    } else {
        model_row.set_sensitive(false);
    }

    let favorites_header = libadwaita::ActionRow::new();
    favorites_header.set_title("Favorite models");
    favorites_header.set_subtitle("Click a favorite to select it; use − to remove it");
    let add_favorite = gtk::Button::from_icon_name("list-add-symbolic");
    add_favorite.add_css_class("flat");
    add_favorite.set_valign(gtk::Align::Center);
    add_favorite.set_tooltip_text(Some("Add the selected model to favorites"));
    add_favorite.set_sensitive(!models.is_empty());
    favorites_header.add_suffix(&add_favorite);

    let favorites_flow = gtk::FlowBox::new();
    favorites_flow.set_max_children_per_line(3);
    favorites_flow.set_min_children_per_line(1);
    favorites_flow.set_column_spacing(6);
    favorites_flow.set_row_spacing(6);
    favorites_flow.set_halign(gtk::Align::Fill);
    favorites_flow.set_selection_mode(gtk::SelectionMode::None);
    let favorites_row = libadwaita::PreferencesRow::new();
    favorites_row.set_child(Some(&favorites_flow));
    favorites_row.set_visible(false);

    let favorites_holder: Rc<RefCell<Option<RefreshFn>>> = Rc::new(RefCell::new(None));
    let refresh_favorites: RefreshFn = {
        let favorites_flow = favorites_flow.clone();
        let favorites_row = favorites_row.clone();
        let model_row = model_row.clone();
        let models = models.clone();
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        let favorites_holder = Rc::clone(&favorites_holder);
        Rc::new(move || {
            let favorites = config::assistant_favorite_models();
            favorites_flow.remove_all();
            favorites_row.set_visible(!favorites.is_empty());
            for favorite in favorites {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                chip.add_css_class("chip");
                chip.set_hexpand(true);

                let select = gtk::Button::new();
                select.add_css_class("flat");
                select.set_hexpand(true);
                select.set_sensitive(
                    models
                        .iter()
                        .any(|model| model_name_matches(&favorite, model)),
                );
                let name = gtk::Label::new(Some(&favorite));
                name.set_xalign(0.0);
                name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                select.set_child(Some(&name));

                let remove = gtk::Button::from_icon_name("window-close-symbolic");
                remove.add_css_class("flat");
                remove.add_css_class("circular");
                remove.set_tooltip_text(Some("Remove from favorites"));
                chip.append(&select);
                chip.append(&remove);
                favorites_flow.append(&chip);

                {
                    let favorite = favorite.clone();
                    let model_row = model_row.clone();
                    let models = models.clone();
                    select.connect_clicked(move |_| {
                        if let Some(index) = models
                            .iter()
                            .position(|model| model_name_matches(&favorite, model))
                        {
                            model_row.set_selected(index as u32);
                        }
                    });
                }
                {
                    let favorite = favorite.clone();
                    let favorites_holder = Rc::clone(&favorites_holder);
                    let status_row = status_row.clone();
                    let status_label = status_label.clone();
                    remove.connect_clicked(move |_| {
                        let result = config::persist_assistant_favorite_remove(&favorite);
                        set_persist_status(&status_row, &status_label, result, ApplyMode::Favorite);
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

    {
        let model_row = model_row.clone();
        let models = models.clone();
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        model_row.clone().connect_selected_notify(move |_| {
            let Some(selected) = selected_string(&model_row) else {
                return;
            };
            if !models.iter().any(|model| model == &selected) {
                return;
            }
            let result = config::persist_assistant_value(
                "assistant",
                "default_model",
                Value::String(selected),
            );
            set_persist_status(&status_row, &status_label, result, ApplyMode::Live);
        });
    }

    {
        let model_row = model_row.clone();
        let models = models.clone();
        let favorites_holder = Rc::clone(&favorites_holder);
        let add_favorite = add_favorite.clone();
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        add_favorite.connect_clicked(move |_| {
            let Some(selected) = selected_string(&model_row) else {
                return;
            };
            if !models.iter().any(|model| model == &selected) {
                return;
            }
            let result = config::persist_assistant_favorite_add(&selected);
            set_persist_status(&status_row, &status_label, result, ApplyMode::Favorite);
            if let Some(refresh) = favorites_holder.borrow().as_ref() {
                refresh();
            }
        });
    }

    group.add(&model_row);
    group.add(&favorites_header);
    group.add(&favorites_row);
    page.add(&group);
}

fn build_decision_model_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
    mut models: Vec<String>,
) {
    models.sort();
    models.dedup();
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Decision model");
    group.set_description(Some(
        "Only Ollama models marked as decision-capable are listed. The selection is saved but does not affect assistant behavior yet.",
    ));
    let row = libadwaita::ComboRow::new();
    row.set_title("Model");
    row.set_subtitle("Decision-model routing is not connected yet");
    let mut labels = vec!["None".to_string()];
    labels.extend(models.iter().cloned());
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let choices = gtk::StringList::new(&label_refs);
    row.set_model(Some(&choices));
    let configured = config::assistant_decision_model();
    let selected = models
        .iter()
        .position(|model| model_name_matches(&configured, model))
        .map(|index| index + 1)
        .unwrap_or(0);
    if configured != "none" && selected == 0 {
        row.set_subtitle(
            "Saved model is unavailable or not decision-capable; choose None or an installed model",
        );
    }
    row.set_selected(selected as u32);
    {
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        let models = models.clone();
        row.connect_selected_notify(move |row| {
            let model = match row.selected() {
                0 => "none".to_string(),
                index => match models.get(index as usize - 1) {
                    Some(model) => model.clone(),
                    None => return,
                },
            };
            let result = config::persist_assistant_decision_model(&model);
            set_persist_status(&status_row, &status_label, result, ApplyMode::Placeholder);
        });
    }
    group.add(&row);
    page.add(&group);
}

fn build_tools_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Tool permissions");
    group.set_description(Some(
        "Allow runs a tool automatically, Ask requires your confirmation, and Deny disables it. Changes reload live.",
    ));
    let tools = ipc::assistant_tools().unwrap_or_else(|error| {
        eprintln!("[settings] could not list Assistant tools: {error}");
        config::assistant_tool_permissions()
    });
    for (tool, permission) in tools {
        let row = libadwaita::ComboRow::new();
        row.set_title(&tool.replace('_', " "));
        row.set_subtitle(&tool);
        let levels = ["allow", "ask", "deny"];
        let level_model = gtk::StringList::new(&levels);
        row.set_model(Some(&level_model));
        row.set_selected(
            levels
                .iter()
                .position(|level| *level == permission)
                .unwrap_or(1) as u32,
        );
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        row.connect_selected_notify(move |row| {
            let Some(level) = selected_string(row) else {
                return;
            };
            let result = config::persist_assistant_value("tools", &tool, Value::String(level));
            set_persist_status(&status_row, &status_label, result, ApplyMode::Live);
        });
        group.add(&row);
    }
    page.add(&group);
}

fn build_memory_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Long-term memory");
    group.set_description(Some(
        "Memory and summarization settings are saved immediately; restart Assistant to apply them.",
    ));

    for (key, title, subtitle, fallback) in [
        (
            "enabled_local",
            "Use memory with local models",
            "Allow local models to read and write long-term memory",
            true,
        ),
        (
            "enabled_cloud",
            "Use memory with cloud models",
            "Allow cloud models to read and write long-term memory",
            false,
        ),
        (
            "summarize_on_idle",
            "Summarize when idle",
            "Save a summary when the conversation becomes idle",
            true,
        ),
        (
            "summarize_on_stop",
            "Summarize when stopped",
            "Save a summary when the assistant session is stopped",
            true,
        ),
    ] {
        let row = libadwaita::SwitchRow::new();
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.set_active(config::assistant_bool("memory", key, fallback));
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        let key = key.to_string();
        row.connect_active_notify(move |row| {
            let result =
                config::persist_assistant_value("memory", &key, Value::Boolean(row.is_active()));
            set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
        });
        group.add(&row);
    }

    for (key, title, subtitle, min, max, step, fallback) in [
        (
            "max_summary_chars",
            "Maximum summary length",
            "Maximum characters retained in a memory summary",
            200.0,
            10000.0,
            100.0,
            1200.0,
        ),
        (
            "retrieve_top_k",
            "Retrieved memories",
            "Maximum number of relevant memory snippets to include",
            1.0,
            20.0,
            1.0,
            4.0,
        ),
    ] {
        let row = libadwaita::SpinRow::with_range(min, max, step);
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.set_digits(0);
        row.set_value(config::assistant_int("memory", key, fallback as i64) as f64);
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        let key = key.to_string();
        row.connect_value_notify(move |row| {
            let result =
                config::persist_assistant_value("memory", &key, Value::Integer(row.value() as i64));
            set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
        });
        group.add(&row);
    }

    for (key, title, subtitle, fallback) in [
        (
            "qdrant_path",
            "Memory database path",
            "Path to the local Qdrant database (~ is expanded)",
            "~/.local/share/neuropipe/memory/qdrant",
        ),
        (
            "collection",
            "Memory collection",
            "Qdrant collection name",
            "assistant_memory",
        ),
        (
            "embedding_model",
            "Embedding model",
            "Ollama model used to embed memory text",
            DEFAULT_EMBEDDING_MODEL,
        ),
    ] {
        add_string_setting(
            &group,
            status_row,
            status_label,
            "memory",
            key,
            title,
            subtitle,
            fallback,
            ApplyMode::Restart,
        );
    }

    page.add(&group);
}

fn build_instructions_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Instructions");
    group.set_description(Some(
        "Customize the assistant's behavior and tool-use policy. Save changes, then restart Assistant to apply them.",
    ));

    let prompt_view = text_view(&config::assistant_string(
        "instructions",
        "system_prompt",
        config::DEFAULT_ASSISTANT_SYSTEM_PROMPT,
    ));
    let policy_view = text_view(&config::assistant_string(
        "instructions",
        "tool_usage_policy",
        config::DEFAULT_ASSISTANT_TOOL_POLICY,
    ));
    let prompt_row = text_view_row("System prompt", &prompt_view);
    let policy_row = text_view_row("Tool usage policy", &policy_view);
    group.add(&prompt_row);
    group.add(&policy_row);

    let save_row = libadwaita::ActionRow::new();
    save_row.set_title("Save instructions");
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    save_button.set_valign(gtk::Align::Center);
    save_row.add_suffix(&save_button);
    group.add(&save_row);

    let status_row = status_row.clone();
    let status_label = status_label.clone();
    save_button.connect_clicked(move |_| {
        let result = config::persist_assistant_instructions(
            &buffer_text(&prompt_view.buffer()),
            &buffer_text(&policy_view.buffer()),
        );
        set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
    });

    page.add(&group);
}

#[allow(deprecated)] // GTK 4.12 supports this native chooser; FileDialog requires GTK 4.10.
fn build_search_folders_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("File search folders");
    group.set_description(Some(
        "Folders available to the file_search tool. Changes are read on its next run. Removing all folders restores the standard home-folder fallback.",
    ));

    let header = libadwaita::ActionRow::new();
    header.set_title("Allowed folders");
    header.set_subtitle("The file_search tool is restricted to these folders");
    let add_button = gtk::Button::from_icon_name("list-add-symbolic");
    add_button.add_css_class("flat");
    add_button.set_valign(gtk::Align::Center);
    add_button.set_tooltip_text(Some("Add a folder"));
    header.add_suffix(&add_button);

    let flow = gtk::FlowBox::new();
    flow.set_max_children_per_line(2);
    flow.set_min_children_per_line(1);
    flow.set_column_spacing(6);
    flow.set_row_spacing(6);
    flow.set_halign(gtk::Align::Fill);
    flow.set_selection_mode(gtk::SelectionMode::None);
    let flow_row = libadwaita::PreferencesRow::new();
    flow_row.set_child(Some(&flow));

    let folders_holder: Rc<RefCell<Option<RefreshFn>>> = Rc::new(RefCell::new(None));
    let refresh_folders: RefreshFn = {
        let flow = flow.clone();
        let flow_row = flow_row.clone();
        let folders_holder = Rc::clone(&folders_holder);
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        Rc::new(move || {
            let folders = config::assistant_search_folders();
            flow.remove_all();
            flow_row.set_visible(!folders.is_empty());
            for folder in folders {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                chip.add_css_class("chip");
                chip.set_hexpand(true);
                let label = gtk::Label::new(Some(&folder));
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_max_width_chars(42);
                label.set_margin_start(8);
                let remove = gtk::Button::from_icon_name("window-close-symbolic");
                remove.add_css_class("flat");
                remove.add_css_class("circular");
                remove.set_tooltip_text(Some("Remove folder"));
                chip.append(&label);
                chip.append(&remove);
                flow.append(&chip);

                let status_row = status_row.clone();
                let status_label = status_label.clone();
                let folders_holder = Rc::clone(&folders_holder);
                remove.connect_clicked(move |_| {
                    let result = config::persist_assistant_search_folder_remove(&folder);
                    set_persist_status(
                        &status_row,
                        &status_label,
                        result,
                        ApplyMode::SearchFolders,
                    );
                    if let Some(refresh) = folders_holder.borrow().as_ref() {
                        refresh();
                    }
                });
            }
        })
    };
    *folders_holder.borrow_mut() = Some(Rc::clone(&refresh_folders));
    refresh_folders();

    {
        let folders_holder = Rc::clone(&folders_holder);
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        add_button.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Add file search folder"),
                None::<&gtk::Window>,
                gtk::FileChooserAction::SelectFolder,
                Some("Add"),
                Some("Cancel"),
            );
            let folders_holder = Rc::clone(&folders_holder);
            let status_row = status_row.clone();
            let status_label = status_label.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(path) = dialog.file().and_then(|file| file.path()) {
                        let folder = path.to_string_lossy().into_owned();
                        let result = config::persist_assistant_search_folder_add(&folder);
                        set_persist_status(
                            &status_row,
                            &status_label,
                            result,
                            ApplyMode::SearchFolders,
                        );
                        if let Some(refresh) = folders_holder.borrow().as_ref() {
                            refresh();
                        }
                    }
                }
                dialog.hide();
            });
            dialog.show();
        });
    }

    group.add(&header);
    group.add(&flow_row);
    page.add(&group);
}

fn build_ducking_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Media ducking");
    group.set_description(Some(
        "Lower playing media while you speak. Changes require an Assistant service restart.",
    ));
    let row = libadwaita::SwitchRow::new();
    row.set_title("Duck media while listening");
    row.set_subtitle("Temporarily lower the active player's volume during speech");
    row.set_active(config::assistant_bool("assistant", "duck_media", true));
    {
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        row.connect_active_notify(move |row| {
            let result = config::persist_assistant_value(
                "assistant",
                "duck_media",
                Value::Boolean(row.is_active()),
            );
            set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
        });
    }
    group.add(&row);

    let volume = libadwaita::SpinRow::with_range(0.0, 1.0, 0.05);
    volume.set_title("Ducked volume");
    volume.set_subtitle("Playback volume while listening (0.0–1.0)");
    volume.set_digits(2);
    volume.set_value(config::assistant_float("assistant", "duck_volume", 0.1));
    {
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        volume.connect_value_notify(move |row| {
            let result = config::persist_assistant_value(
                "assistant",
                "duck_volume",
                Value::Float(row.value()),
            );
            set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
        });
    }
    group.add(&volume);
    page.add(&group);
}

fn build_general_group(
    page: &libadwaita::PreferencesPage,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Conversation");
    group.set_description(Some(
        "General assistant behavior. Changes require an Assistant service restart.",
    ));
    let timeout = libadwaita::SpinRow::with_range(1.0, 86400.0, 60.0);
    timeout.set_title("History idle timeout");
    timeout.set_subtitle("Seconds before a conversation is considered idle and reset");
    timeout.set_digits(0);
    timeout.set_value(config::assistant_int("assistant", "history_idle_timeout_sec", 3600) as f64);
    timeout.connect_value_notify({
        let status_row = status_row.clone();
        let status_label = status_label.clone();
        move |row| {
            let result = config::persist_assistant_value(
                "assistant",
                "history_idle_timeout_sec",
                Value::Integer(row.value() as i64),
            );
            set_persist_status(&status_row, &status_label, result, ApplyMode::Restart);
        }
    });
    group.add(&timeout);
    page.add(&group);
}

fn build_test_group(page: &libadwaita::PreferencesPage) {
    let group = libadwaita::PreferencesGroup::new();
    group.set_title("Test Assistant");
    group.set_description(Some(
        "Start a voice session, interrupt the current response, or return the assistant to idle.",
    ));

    let mode_row = libadwaita::ActionRow::new();
    mode_row.set_title("Assistant state");
    mode_row.set_subtitle("Connecting to the Assistant service…");

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_halign(gtk::Align::Center);
    let mut action_buttons = Vec::new();
    for (label, command) in [
        ("Mode 1", "mode1"),
        ("Mode 2", "mode2"),
        ("Interrupt", "interrupt"),
        ("Stop", "stop"),
    ] {
        let button = gtk::Button::with_label(label);
        controls.append(&button);
        button.set_tooltip_text(Some(match command {
            "mode1" => "Start half-duplex voice conversation",
            "mode2" => "Start full-duplex voice conversation",
            "interrupt" => "Interrupt current assistant output",
            _ => "Stop the session and return to idle",
        }));
        action_buttons.push((button, command.to_string()));
    }
    let controls_row = libadwaita::PreferencesRow::new();
    controls_row.set_child(Some(&controls));

    let loaded_header = libadwaita::ActionRow::new();
    loaded_header.set_title("Loaded Ollama models");
    loaded_header.set_subtitle("Live process information, similar to `ollama ps`");
    let loaded_label = gtk::Label::new(Some("Checking Ollama…"));
    loaded_label.set_xalign(0.0);
    loaded_label.set_wrap(true);
    loaded_label.set_selectable(true);
    loaded_label.set_max_width_chars(90);
    let loaded_row = libadwaita::PreferencesRow::new();
    loaded_row.set_child(Some(&loaded_label));

    let action_status = gtk::Label::new(Some(""));
    action_status.add_css_class("dim-label");
    action_status.set_wrap(true);
    action_status.set_xalign(0.0);
    let action_status_row = libadwaita::PreferencesRow::new();
    action_status_row.set_child(Some(&action_status));
    action_status_row.set_visible(false);

    group.add(&mode_row);
    group.add(&controls_row);
    group.add(&loaded_header);
    group.add(&loaded_row);
    group.add(&action_status_row);
    page.add(&group);

    let (status_tx, mut status_rx) = futures_channel::mpsc::unbounded::<ipc::AssistantLiveStatus>();
    {
        let mode_row = mode_row.clone();
        let loaded_label = loaded_label.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Some((state, models)) = status_rx.next().await {
                match state {
                    Ok(state) => {
                        let mode = state
                            .get("mode")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("UNKNOWN");
                        let busy = state
                            .get("busy")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let model = state
                            .get("model")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        let mut detail = if busy { "Working" } else { "Idle" }.to_string();
                        if !model.is_empty() {
                            detail.push_str(" · ");
                            detail.push_str(model);
                        }
                        mode_row.set_title(&format!("Assistant state: {mode}"));
                        mode_row.set_subtitle(&detail);
                    }
                    Err(_) => {
                        mode_row.set_title("Assistant state: unavailable");
                        mode_row.set_subtitle("Start the Assistant service to control a session");
                    }
                }
                match models {
                    Ok(models) if models.is_empty() => {
                        loaded_label.set_text("No Ollama models are currently loaded.");
                    }
                    Ok(models) => loaded_label.set_text(
                        &models
                            .iter()
                            .map(loaded_model_summary)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Err(error) => {
                        loaded_label.set_text(&format!("Ollama status unavailable: {error}"))
                    }
                }
            }
        });
    }
    ipc::spawn_assistant_status_poller(status_tx);

    let (action_tx, mut action_rx) =
        futures_channel::mpsc::unbounded::<(String, Result<serde_json::Value, String>)>();
    {
        let action_status = action_status.clone();
        let action_status_row = action_status_row.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Some((command, result)) = action_rx.next().await {
                let message = match result {
                    Ok(_) => match command.as_str() {
                        "mode1" => "Mode 1 started.",
                        "mode2" => "Mode 2 started.",
                        "interrupt" => "Assistant interrupted.",
                        "stop" => "Assistant stopped.",
                        _ => "Assistant command completed.",
                    }
                    .to_string(),
                    Err(error) => format!("Assistant command failed: {error}"),
                };
                action_status.set_text(&message);
                action_status_row.set_visible(true);
            }
        });
    }

    for (button, command) in action_buttons {
        let action_tx = action_tx.clone();
        let action_status = action_status.clone();
        let action_status_row = action_status_row.clone();
        button.connect_clicked(move |_| {
            action_status.set_text(&format!("Sending {command}…"));
            action_status_row.set_visible(true);
            ipc::spawn_assistant_action(command.clone(), action_tx.clone());
        });
    }
}

fn loaded_model_summary(model: &serde_json::Value) -> String {
    if let Some(display) = model.get("display").and_then(serde_json::Value::as_str) {
        return display.to_string();
    }
    let mut parts = Vec::new();
    let name = model
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Unknown model");
    parts.push(name.to_string());

    let size = model
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if size > 0 {
        parts.push(format_size(size));
        let vram = model
            .get("size_vram")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let gpu_percent = (vram as f64 / size as f64 * 100.0).round() as u64;
        let processor = if gpu_percent == 0 {
            "CPU".to_string()
        } else if gpu_percent >= 98 {
            "GPU".to_string()
        } else {
            format!("{gpu_percent}% GPU")
        };
        parts.push(processor);
    }
    if let Some(context) = model
        .get("context_length")
        .and_then(serde_json::Value::as_u64)
    {
        parts.push(format!("context {}", format_count(context)));
    }
    if let Some(details) = model.get("details") {
        if let Some(parameters) = details
            .get("parameter_size")
            .and_then(serde_json::Value::as_str)
        {
            parts.push(parameters.to_string());
        }
        if let Some(quantization) = details
            .get("quantization_level")
            .and_then(serde_json::Value::as_str)
        {
            parts.push(quantization.to_string());
        }
    }
    if let Some(expiration) = model.get("expires_at").and_then(serde_json::Value::as_str) {
        if !expiration.is_empty() {
            parts.push(format!("until {expiration}"));
        }
    }
    parts.join(" · ")
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit - 1])
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn add_string_setting(
    group: &libadwaita::PreferencesGroup,
    status_row: &libadwaita::PreferencesRow,
    status_label: &gtk::Label,
    section: &'static str,
    key: &'static str,
    title: &'static str,
    subtitle: &'static str,
    fallback: &'static str,
    apply: ApplyMode,
) {
    let row = libadwaita::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    let entry = gtk::Entry::new();
    entry.set_text(&config::assistant_string(section, key, fallback));
    entry.set_hexpand(true);
    entry.set_halign(gtk::Align::End);
    entry.set_valign(gtk::Align::Center);
    row.add_suffix(&entry);
    row.set_activatable_widget(Some(&entry));
    let status_row = status_row.clone();
    let status_label = status_label.clone();
    entry.connect_activate(move |entry| {
        let result = config::persist_assistant_value(
            section,
            key,
            Value::String(entry.text().trim().to_string()),
        );
        set_persist_status(&status_row, &status_label, result, apply);
    });
    group.add(&row);
}

fn text_view(text: &str) -> gtk::TextView {
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(8);
    view.set_right_margin(8);
    view.buffer().set_text(text);
    view
}

fn text_view_row(title: &str, view: &gtk::TextView) -> libadwaita::PreferencesRow {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 6);
    group.set_margin_top(8);
    group.set_margin_bottom(8);
    group.set_margin_start(12);
    group.set_margin_end(12);
    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("heading");
    group.append(&label);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(110)
        .max_content_height(220)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    scroll.set_child(Some(view));
    group.append(&scroll);
    let row = libadwaita::PreferencesRow::new();
    row.set_child(Some(&group));
    row
}

fn buffer_text(buffer: &gtk::TextBuffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .to_string()
}

fn selected_string(row: &libadwaita::ComboRow) -> Option<String> {
    let model = row.model()?.downcast::<gtk::StringList>().ok()?;
    model.string(row.selected()).map(|value| value.to_string())
}

fn model_name_matches(configured: &str, available: &str) -> bool {
    configured == available
        || configured.strip_suffix(":latest") == Some(available)
        || available.strip_suffix(":latest") == Some(configured)
}

#[cfg(test)]
mod tests {
    use super::{format_count, format_size, loaded_model_summary};
    use serde_json::json;

    #[test]
    fn formats_ollama_loaded_model_details() {
        let model = json!({
            "name": "test-model",
            "size": 1024,
            "size_vram": 1024,
            "context_length": 4096,
            "details": {
                "parameter_size": "7B",
                "quantization_level": "Q4_K_M"
            },
            "expires_at": "soon"
        });
        assert_eq!(
            loaded_model_summary(&model),
            "test-model · 1.0 KB · GPU · context 4,096 · 7B · Q4_K_M · until soon"
        );
    }

    #[test]
    fn formats_sizes_and_context_counts() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }
}
