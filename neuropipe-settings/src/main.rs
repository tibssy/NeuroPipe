mod services;
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
             .status-inactive { color: #e01b24; }",
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
