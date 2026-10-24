use crate::LOGGER;
use crate::backend::window_manager_hypr::{HyprlandComms, WindowManagerEvent};
use crate::backend::{Backend, BackendChannels};
use crate::ui::action_centre::ActionCentre;
use crossbeam_channel::TryRecvError;
use glib::ControlFlow;
use glib::prelude::Cast;
use gtk::gdk::Display;
use gtk::prelude::{BoxExt, ButtonExt, DisplayExt, GtkWindowExt, ListModelExt, WidgetExt};
use gtk::{Align, Application, ApplicationWindow, Button, Orientation, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use jmb_logger_rs::*;
use mpris::PlayerFinder;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

mod action_centre;
mod systray;
mod title_panel;
mod utilities_panel;
mod widgets;
mod workspaces_panel;

pub fn create_bars(app: &Application, backend: Backend) {
    let display = Display::default().expect("Couldn't find GDK Display");
    let app_clone = app.clone();
    let bars: Mutex<HashMap<u32, ApplicationWindow>> = Mutex::new(HashMap::new());
    let ac_channels = backend.subscribe();
    let action_centre = ActionCentre::create_action_centre(app.clone(), ac_channels);
    for monitor in 0..display.monitors().n_items() {
        debug!(&LOGGER, "Opening bar {monitor}");
        let channels = backend.subscribe();
        bars.lock().expect("Couldn't get active bars").insert(
            monitor,
            create_bar(
                app_clone.clone(),
                &display,
                monitor,
                action_centre.clone(),
                channels,
            ),
        );
    }
    display
        .monitors()
        .connect_items_changed(move |_list, pos, removed, added| {
            if removed == 1 {
                debug!(&LOGGER, "Detected Monitor disconnect {pos}");
                if let Some(bar) = bars.lock().expect("Couldn't get active bars").remove(&pos) {
                    bar.close();
                }
            } else if added == 1 {
                debug!(&LOGGER, "Detected new monitor: {pos}");
                let channels = backend.subscribe();
                bars.lock().expect("Couldn't get active bars").insert(
                    pos,
                    create_bar(
                        app_clone.clone(),
                        &display,
                        pos,
                        action_centre.clone(),
                        channels,
                    ),
                );
            }
        });
}

fn create_bar(
    app: Application,
    display: &Display,
    monitor_idx: u32,
    action_centre: ActionCentre,
    mut channels: BackendChannels,
) -> ApplicationWindow {
    // Take the Receiver and replace it with none so that it can be only used in one place
    let hypr_rx = channels
        .take_hypr_rx()
        .expect("Tried taking unavailable Hyprland Channel Receiver");

    let player_opt = if let Ok(player_finder) = PlayerFinder::new() {
        player_finder.find_active().ok()
    } else {
        None
    };

    let window = ApplicationWindow::builder()
        .application(&app)
        .title("JmbBar")
        .default_width(500)
        .default_height(30)
        .css_classes(vec!["bar"])
        .build();

    let monitors = display.monitors();

    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_margin(Edge::Top, 20);
    if let Some(monitor) = monitors.item(monitor_idx)
        && let Ok(monitor) = monitor.downcast::<gdk::Monitor>()
    {
        window.set_monitor(Some(&monitor));
    }

    window.set_exclusive_zone(30);

    let mut buttons: HashMap<i32, Button> = HashMap::new();
    let (workspaces, submap) = workspaces_panel::build(
        &mut buttons,
        window.monitor().and_then(|monitor| monitor_index(&monitor)),
    );

    let title_button: Option<Button>;
    let title = title_panel::build();

    let utilities = utilities_panel::build(&app, action_centre, channels, monitor_idx, display);

    let dock = gtk::Box::new(Orientation::Horizontal, 10);
    dock.add_css_class("dock");
    dock.set_vexpand(true);
    dock.set_valign(Align::Fill);
    dock.append(&workspaces);
    dock.append(&title);
    dock.append(&utilities);

    title_button = Some(title);
    title_panel::manage_title(&title_button, &player_opt);

    window.set_keyboard_mode(KeyboardMode::None);
    window.set_child(Some(&dock));
    window.present();

    let window_clone = window.clone();

    glib::timeout_add_local(Duration::from_millis(50), move || {
        loop {
            match hypr_rx.try_recv() {
                Ok(event) => {
                    handle_wm_event(
                        &mut buttons,
                        &submap,
                        &title_button,
                        event,
                        window_clone.monitor(),
                    );
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return ControlFlow::Break;
                }
            }
        }

        title_panel::manage_title(&title_button, &player_opt);

        ControlFlow::Continue
    });
    window
}

fn monitor_index(target: &gdk::Monitor) -> Option<u32> {
    let display = Display::default()?;
    let monitors = display.monitors();

    (0..monitors.n_items()).find(|&i| {
        monitors
            .item(i)
            .and_then(|obj| obj.downcast::<gdk::Monitor>().ok())
            .is_some_and(|monitor| monitor == *target)
    })
}

fn handle_wm_event(
    buttons: &mut HashMap<i32, Button>,
    submap_label: &Button,
    title_button: &Option<Button>,
    event: WindowManagerEvent,
    current_monitor: Option<gdk::Monitor>,
) {
    trace!(&LOGGER, "Received WM Event {event:?}");
    match event {
        WindowManagerEvent::WorkspaceChanged(workspace) => {
            for (&id, button) in buttons {
                if id == workspace {
                    button.add_css_class("active");
                } else {
                    button.remove_css_class("active");
                }
            }
            title_panel::set_title(&HyprlandComms::get_current_title(), title_button);
        }
        WindowManagerEvent::WorkspaceAdded(workspace, monitor) => {
            for (&id, button) in buttons {
                if id == workspace
                    && current_monitor.as_ref().is_none_or(|cur_mon| {
                        monitor_index(cur_mon).is_some_and(|idx| idx == monitor as u32)
                    })
                {
                    button.set_visible(true);
                }
            }
        }
        WindowManagerEvent::WorkspaceRemoved(workspace) => {
            for (&id, button) in buttons {
                if id == workspace {
                    button.set_visible(false);
                }
            }
        }
        WindowManagerEvent::WindowFocusChanged(title) => {
            title_panel::set_title(&title, title_button);
        }
        WindowManagerEvent::SubmapChange(submap) => match submap {
            Some(submap) => {
                submap_label.set_label(&submap);
                submap_label.add_css_class("active");
            }
            None => {
                submap_label.set_label("");
                submap_label.remove_css_class("active");
            }
        },
    }
}
