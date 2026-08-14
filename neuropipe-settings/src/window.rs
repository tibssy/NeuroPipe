use gtk::glib;
use gtk::prelude::*;
use libadwaita::{prelude::*, Application, ApplicationWindow};

use crate::services::Service;

fn placeholder_body(title: &str, detail: &str) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(32);
    body.set_margin_bottom(32);
    body.set_margin_start(32);
    body.set_margin_end(32);

    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-2");
    heading.set_halign(gtk::Align::Start);
    body.append(&heading);

    let sub = gtk::Label::new(Some(detail));
    sub.add_css_class("dim-label");
    sub.set_wrap(true);
    sub.set_xalign(0.0);
    body.append(&sub);

    body
}

fn build_tab(service: Service) -> gtk::Widget {
    if service == Service::Tts {
        return crate::tts_tab::build_tts_tab();
    }

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    let (title, detail) = match service {
        Service::Stt => (
            "Speech-to-Text",
            "Placeholder for STT settings (model, VAD, turn-end detection).",
        ),
        Service::Assistant => (
            "Assistant",
            "Placeholder for Assistant settings (model, memory, tools, ducking).",
        ),
        Service::Tts => unreachable!("TTS handled above"),
    };
    scroller.set_child(Some(&placeholder_body(title, detail)));
    scroller.upcast()
}

/// Bottom status strip: one indicator per installed service, showing whether
/// its systemd unit is currently running.
fn build_status_bar(services: &[Service]) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    bar.set_margin_top(4);
    bar.set_margin_bottom(4);
    bar.set_margin_start(12);
    bar.set_margin_end(12);

    for service in services {
        let active = service.is_active();
        let dot = gtk::Label::new(Some("●"));
        dot.set_css_classes(&[if active { "status-active" } else { "status-inactive" }]);
        let label = gtk::Label::new(Some(&format!("{}: {}", service.label(), if active { "running" } else { "stopped" })));
        label.add_css_class("dim-label");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.append(&dot);
        row.append(&label);
        bar.append(&row);
    }
    bar
}

pub fn build_window(app: &Application) -> ApplicationWindow {
    let win = ApplicationWindow::builder()
        .application(app)
        .title("NeuroPipe Settings")
        .default_width(720)
        .default_height(520)
        .build();

    // Dynamic tabs: one per installed service, fixed order, missing ones skipped.
    let tab_view = libadwaita::TabView::new();
    let installed = Service::installed();
    for service in &installed {
        let page = tab_view.append(&build_tab(*service));
        page.set_title(service.label());
    }

    // Close buttons are hidden via CSS; deny any close-page requests too so the
    // (invisible) button's click hotspot can never close a tab.
    tab_view.connect_close_page(|_, _| glib::Propagation::Stop);

    let tab_bar = libadwaita::TabBar::new();
    tab_bar.set_view(Some(&tab_view));
    tab_bar.set_expand_tabs(true);

    let toolbar = libadwaita::ToolbarView::new();
    toolbar.add_top_bar(&tab_bar);
    toolbar.set_content(Some(&tab_view));

    // If nothing is installed, show a pointer to the installer instead of
    // empty tabs.
    if installed.is_empty() {
        tab_bar.set_visible(false);
        let notice = placeholder_body(
            "No NeuroPipe services installed",
            "Run install.sh to choose which services to install, then reopen this app.",
        );
        toolbar.set_content(Some(&notice));
    } else {
        toolbar.add_bottom_bar(&build_status_bar(&installed));
    }

    win.set_content(Some(&toolbar));
    win
}
