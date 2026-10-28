use crate::backend::Backend;
mod backend;
mod css;
mod ui;

use glib::clone;
use gtk::prelude::*;
use gtk::{Application, CssProvider, gdk, gio};
use jmb_logger_rs::*;
use notify::RecommendedWatcher;
use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

pub const LOGGER: Logger = Logger::create(Level::DEBUG);
pub const APP_NAME: &str = "net.jmb19905.jmb-bar";

fn main() {
    info!(&LOGGER, "Starting jmb-bar");

    gio::resources_register_include!("templates.gresource").expect("Failed to register resources");

    let app = Application::builder().application_id(APP_NAME).build();

    let watcher_slot: Rc<RefCell<Option<RecommendedWatcher>>> = Rc::new(RefCell::new(None));

    let _ = Command::new("pkill hypridle").output(); // Kill existing hypridle processes

    let backend = Backend::start();

    app.connect_startup(move |_| {
        let provider = CssProvider::new();
        if let Some(css) = css::parse_css() {
            provider.load_from_string(&css);
        }
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("Could not connect to display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let watcher = css::watch_css(move |css| provider.load_from_string(&css));
        *watcher_slot.borrow_mut() = Some(watcher);
    });

    app.connect_activate(clone!(
        #[strong]
        backend,
        move |app| {
            trace!(&LOGGER, "Starting bars");
            ui::create_bars(app, backend.clone());
        }
    ));

    app.run();
}
