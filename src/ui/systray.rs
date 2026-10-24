use crate::LOGGER;
use glib::ControlFlow;
use gtk::Orientation;
use jmb_logger_rs::*;
use rustsni::{TrayEvent, TrayHost};
use std::time::Duration;

pub fn _build() -> gtk::Box {
    let systray = gtk::Box::new(Orientation::Horizontal, 0);
    let host = TrayHost::new();

    if let Ok(mut host) = host {
        glib::timeout_add_local(
            Duration::from_millis(1_000),
            move || {
                for event in host.poll().unwrap() {
                    match event {
                        TrayEvent::ItemAdded(id) | TrayEvent::ItemChanged(id) => {
                            if let Some(item) = host.items().get(&id) {
                                println!("tray: {} — {}", item.title, item.status);
                                println!("  category: {}", item.category);
                                println!("  has menu: {}", item.has_menu());
                            }
                        }
                        TrayEvent::ItemRemoved(id) => {
                            println!("item gone: {id}");
                        }
                        TrayEvent::HostShutdown => break,
                        _ => {}
                    }
                }
                ControlFlow::Continue
            }
        );
    } else {
        error!(&LOGGER, "Couldn't get SystemTray");
    }

    systray
}