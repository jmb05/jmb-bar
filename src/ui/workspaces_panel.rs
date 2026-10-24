use crate::LOGGER;
use crate::backend::window_manager_hypr::HyprlandComms;
use gtk::prelude::{BoxExt, ButtonExt, WidgetExt};
use gtk::{Align, Button, Orientation};
use hyprland::data::Workspace;
use hyprland::prelude::HyprDataActive;
use jmb_logger_rs::*;
use std::collections::HashMap;

pub(crate) fn build(buttons: &mut HashMap<i32, Button>, monitor: Option<u32>) -> (gtk::Box, Button) {
    let workspaces = gtk::Box::new(Orientation::Horizontal, 0);
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);
    workspaces.set_vexpand(true);
    workspaces.set_valign(Align::Fill);
    workspaces.set_hexpand(true);
    workspaces.add_css_class("workspaces");

    for workspace in 1..10 {
        let button = Button::with_label(&format!("{workspace}"));
        button.set_vexpand(true);
        button.add_css_class("workspace-button");
        button.set_visible(false);

        let initial_workspace = Workspace::get_active()
            .ok()
            .map(|workspace| workspace.id().unwrap_or(1) as i32)
            .unwrap_or(1);

        let is_active_ws_on_current_monitor = Workspace::get_active()
            .ok()
            .map(|ws| ws.monitor_id)
            .flatten()
            .is_some_and(|id| monitor.is_some_and(|other_id| id == other_id as i128));

        if workspace == initial_workspace && is_active_ws_on_current_monitor {
            button.add_css_class("active");
            button.set_visible(true);
        }
        if HyprlandComms::workspace_added(workspace, monitor.map(|monitor| monitor as i128)) {
            button.set_visible(true);
        }
        button.connect_clicked(move |_button| {
            if let Err(err) = HyprlandComms::dispatch(&format!("hl.dsp.focus({{ workspace = {workspace} }})")) {
                warn!(&LOGGER, "Couldn't change workspace focus {}", err.to_string());
            }
        });
        workspaces.append(&button);
        buttons.insert(workspace, button);
    }

    let submap_button = Button::new();
    submap_button.add_css_class("submap");
    submap_button.connect_clicked(|_| {
        if let Err(err) = HyprlandComms::dispatch("hl.dsp.submap('reset')") {
            warn!(&LOGGER, "Couldn't reset submap {}", err.to_string());
        }
    });
    workspaces.append(&submap_button);

    (workspaces, submap_button)
}