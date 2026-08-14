mod config;
mod ipc;
mod services;
mod tts;
mod tts_tab;
mod window;

use gtk::glib;
use gtk::prelude::*;
use libadwaita::Application;

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("io.neuropipe.Settings")
        .build();

    app.connect_activate(|app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(
            ".status-active { color: #2ec27e; } \
             .status-inactive { color: #e01b24; } \
             .status-idle { color: #777777; } \
             .eq-bar { min-width: 3px; min-height: 6px; border-radius: 2px; background-color: #777777; } \
             .eq-active .eq-bar { animation: eq-bounce 1.2s ease-in-out infinite; } \
             .eq-active .eq-bar-0 { animation-delay: -0.0s; } \
             .eq-active .eq-bar-1 { animation-delay: -0.3s; } \
             .eq-active .eq-bar-2 { animation-delay: -0.6s; } \
             .eq-active .eq-bar-3 { animation-delay: -0.9s; } \
             @keyframes eq-bounce { 0%, 100% { min-height: 6px; } 50% { min-height: 22px; } } \
             tab button.tab-close-button { opacity: 0; }",
        );
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("no display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let win = window::build_window(app);
        win.present();
    });

    app.run()
}
