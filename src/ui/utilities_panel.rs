use crate::LOGGER;
use crate::backend::audio::MediaCommand;
use crate::backend::brightness_control::BrightnessMessage;
use crate::backend::idle::{IdleMessage, IdleState};
use crate::backend::network::{NetworkCommand, NetworkState, NetworkType};
use crate::backend::{BackendChannels, json_socket};
use crate::ui::action_centre::ActionCentre;
use crate::ui::widgets::{ExpandableBarItem, IconSlider};
use battery::State;
use glib::prelude::Cast;
use glib::subclass::prelude::ObjectSubclassIsExt;
use glib::{ControlFlow, clone};
use gtk::prelude::{
    ApplicationExt, BoxExt, ButtonExt, DisplayExt, ListModelExt, MonitorExt, ToggleButtonExt,
    WidgetExt,
};
use gtk::{
    Adjustment, Align, Application, Button, Image, Label, Orientation, Revealer,
    RevealerTransitionType, gdk,
};
use jmb_logger_rs::*;
use std::cell::Cell;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

const BRIGHTNESS_ICONS: [&str; 3] = ["", "", ""];
const BATTERY_ICONS: [&str; 5] = ["", "", "", "", ""];
const WIFI_ICON_PATHS: [&str; 3] = [
    "icons/wifi_low.svg",
    "icons/wifi_medium.svg",
    "icons/wifi_full.svg",
];
const VOLUME_ICON_PATHS: [&str; 4] = [
    "icons/volume-off.svg",
    "icons/volume-low.svg",
    "icons/volume.svg",
    "icons/volume-high.svg",
];

pub fn build(
    app: &Application,
    action_centre: ActionCentre,
    mut channels: BackendChannels,
    monitor_idx: u32,
    display: &gdk::Display,
) -> gtk::Box {
    let utilities = gtk::Box::new(Orientation::Horizontal, 0);
    utilities.set_halign(Align::End);
    utilities.set_vexpand(true);
    utilities.set_hexpand(true);
    utilities.set_valign(Align::Fill);
    utilities.add_css_class("utilities");

    let monitors = display.monitors();
    let monitor = if let Some(monitor) = monitors.item(monitor_idx)
        && let Ok(monitor) = monitor.downcast::<gdk::Monitor>()
        && let Some(connector) = monitor.connector()
    {
        String::from(connector)
    } else {
        String::new()
    };
    let monitor_valid = !monitor.is_empty();
    if monitor_valid {
        let _ = channels
            .br_tx()
            .send(BrightnessMessage::Request(monitor.clone()));
    }

    let volume_adjustment = Adjustment::new(60.0, 1.0, 100.0, 1.0, 5.0, 0.0);

    let volume = IconSlider::new(Some(volume_adjustment));
    volume.imp().main_icon.set_icon_path(VOLUME_ICON_PATHS[2]);
    volume.imp().second_icon.set_icon_name("bluetooth-symbolic");
    volume.imp().second_icon.add_child_css_class("bt-icon");

    let media_tx = channels.media_tx();
    let changed_progr = Rc::new(Cell::new(false));
    let changed_progr_clone = changed_progr.clone();
    volume.connect_value_changed(move |slider, button, _| {
        if !changed_progr_clone.get() {
            let _ = media_tx.send(MediaCommand::Cmd(
                slider.value() as u8,
                button.is_active(),
                false,
            ));
        }
    });
    let media_tx = channels.media_tx();
    volume.connect_clicked(move |slider, button, _| {
        if button.is_active() {
            let _ = media_tx.send(MediaCommand::Cmd(slider.value() as u8, true, false));
        } else {
            let _ = media_tx.send(MediaCommand::Cmd(slider.value() as u8, false, false));
        }
    });

    let brightness_adjustment = Adjustment::new(60.0, 1.0, 100.0, 1.0, 5.0, 0.0);
    let brightness = IconSlider::new(Some(brightness_adjustment));
    brightness.imp().main_icon.set_font_icon("");

    let br_tx = channels.br_tx();
    brightness.connect_value_changed(clone!(
        #[strong]
        monitor,
        move |slider, _button, _| {
            if monitor_valid {
                let slider_val = slider.value().max(1.0);
                let _ = br_tx.send(BrightnessMessage::Value(
                    monitor.clone(),
                    slider_val as u8,
                    false,
                ));
                let idx =
                    ((slider_val / 100.0) * (BRIGHTNESS_ICONS.len() - 1) as f64 + 0.5) as usize;
                slider.imp().main_icon.set_font_icon(BRIGHTNESS_ICONS[idx]);
            }
        }
    ));
    let br_tx = channels.br_tx();
    brightness.connect_clicked(clone!(
        #[strong]
        monitor,
        move |slider, _button, _| {
            if monitor_valid {
                let _ = br_tx.send(BrightnessMessage::Value(monitor.clone(), 60, false));
                slider.set_value(60.0);
                let idx = ((60.0 / 100.0) * (BRIGHTNESS_ICONS.len() - 1) as f64 + 0.5) as usize;
                slider.imp().main_icon.set_font_icon(BRIGHTNESS_ICONS[idx]);
            }
        }
    ));

    let caffeine_button = Button::with_label(" ");
    caffeine_button.add_css_class("caffeine");
    let idle_tx = channels.idle_tx();
    caffeine_button.connect_clicked(move |button| {
        if button.has_css_class("on") {
            let _ = idle_tx.send(IdleMessage::Release);
        } else {
            let _ = idle_tx.send(IdleMessage::Inhibit);
        }
    });

    let stats = ExpandableBarItem::new("");
    stats.add_css_class("stats");
    stats.set_spacing(0);
    stats.set_vexpand(true);
    stats.set_valign(Align::Fill);

    let cpu_box = gtk::Box::new(Orientation::Horizontal, 8);
    let cpu_icon = Label::new(Some(""));
    cpu_icon.add_css_class("stat-icon");
    cpu_box.append(&cpu_icon);
    let cpu_label = Label::new(None);
    cpu_box.append(&cpu_label);
    stats.set_button_child(Some(&cpu_box));

    let stats_children = gtk::Box::new(Orientation::Horizontal, 6);
    stats_children.add_css_class("expandable-content");

    let ram_box = gtk::Box::new(Orientation::Horizontal, 6);
    let ram_icon = Label::new(Some(""));
    ram_icon.add_css_class("stat-icon");
    ram_box.append(&ram_icon);
    let ram_label = Label::new(None);
    ram_box.append(&ram_label);
    stats_children.append(&ram_box);

    let disk_box = gtk::Box::new(Orientation::Horizontal, 9);
    let disk_icon = Label::new(Some(""));
    disk_icon.add_css_class("stat-icon");
    disk_box.append(&disk_icon);
    let disk_label = Label::new(None);
    disk_box.append(&disk_label);
    stats_children.append(&disk_box);

    let gpu_box = gtk::Box::new(Orientation::Horizontal, 9);
    let gpu_icon = Image::from_file("icons/gpu-card.svg");
    gpu_icon.add_css_class("stat-icon");
    gpu_box.append(&gpu_icon);
    let gpu_label = Label::new(None);
    gpu_box.append(&gpu_label);
    stats_children.append(&gpu_box);
    stats.set_second_child(stats_children);

    let network_button = Button::new();
    network_button.add_css_class("network");
    let network_button_content = gtk::Box::new(Orientation::Horizontal, 0);
    let network_icon = Image::from_file("../../icons/disconnected.svg");
    network_icon.add_css_class("network-icon");
    network_button_content.append(&network_icon);
    let network_label = Label::new(None);
    network_label.add_css_class("network-label");
    network_label.set_visible(false);
    network_button_content.append(&network_label);
    network_button.set_child(Some(&network_button_content));

    network_button.connect_clicked(clone!(
        #[weak]
        display,
        move |_| {
            if action_centre.window.is_visible() {
                action_centre.hide_action_centre();
            } else {
                action_centre.show_action_centre(monitor_idx, &display);
            }
        }
    ));

    let wg_button = Button::with_label("wg");
    wg_button.add_css_class("vpn");
    let net_tx = channels.net_tx();
    wg_button.connect_clicked(move |_| {
        let _ = net_tx.send(NetworkCommand::ToggleVpn(String::from("wg")));
    });

    let netbird_button = Button::with_label("nb");
    netbird_button.add_css_class("vpn");
    netbird_button.connect_clicked(move |button| {
        let _ = Command::new("netbird")
            .arg(if button.has_css_class("on") {
                "down"
            } else {
                "up"
            })
            .output();
    });

    let tud_button = Button::with_label("tud");
    tud_button.add_css_class("vpn");
    let mvd_button = Button::with_label("mvd");
    mvd_button.add_css_class("vpn");

    let vpn_box = gtk::Box::new(Orientation::Horizontal, 0);
    vpn_box.set_valign(Align::Fill);
    vpn_box.set_vexpand(true);
    vpn_box.append(&wg_button);
    vpn_box.append(&netbird_button);
    //vpn_box.append(&tud_button);
    //vpn_box.append(&mvd_button);

    let vpn_revealer = Revealer::new();
    vpn_revealer.set_transition_type(RevealerTransitionType::SlideRight);
    vpn_revealer.set_transition_duration(500);
    vpn_revealer.set_reveal_child(false);
    vpn_revealer.set_child(Some(&vpn_box));
    vpn_revealer.add_css_class("revealer");

    let vpn_button = Button::with_label("");
    vpn_button.add_css_class("vpn-icon");

    let battery_widget = ExpandableBarItem::new("");
    battery_widget.add_css_class("battery");
    battery_widget.set_spacing(0);
    battery_widget.set_vexpand(true);
    battery_widget.set_valign(Align::Fill);

    let battery_children = gtk::Box::new(Orientation::Horizontal, 0);

    let wattage = Label::new(None);
    battery_children.append(&wattage);
    let remaining_time = Label::new(None);
    battery_children.append(&remaining_time);

    battery_widget.set_second_child(battery_children);

    let time = ExpandableBarItem::new("");
    time.add_css_class("time");
    time.set_spacing(0);
    time.set_vexpand(true);
    time.set_valign(Align::Fill);

    let time_hidden = Label::new(None);
    time.set_second_child(time_hidden.clone());

    let power_button = Button::with_label(" ");
    power_button.add_css_class("power-button");
    power_button.connect_clicked(clone!(
        #[strong]
        app,
        move |_button| {
            app.quit();
            //let _ = Command::new("wlogout").spawn();
        }
    ));

    utilities.append(&volume);
    utilities.append(&brightness);
    utilities.append(&caffeine_button);
    utilities.append(&stats);
    utilities.append(&network_button);
    utilities.append(&vpn_button);
    utilities.append(&vpn_revealer);
    //utilities.append(&systray_box);
    utilities.append(&battery_widget);
    utilities.append(&time);
    utilities.append(&power_button);

    vpn_button.connect_clicked(move |_button| {
        vpn_revealer.set_reveal_child(!vpn_revealer.is_child_revealed());
    });

    glib::timeout_add_local(Duration::from_millis(1000), move || {
        let timestamp = time_format::now().unwrap();
        let time_str = time_format::strftime_local("%H:%M", timestamp).unwrap_or_default();
        time.set_title(&time_str);
        let time_sec = time_format::strftime_local(":%S %Y-%m-%d", timestamp).unwrap_or_default();
        time_hidden.set_label(&time_sec);
        ControlFlow::Continue
    });

    let media_rx = channels.take_media_rx().unwrap();
    let br_rx = channels.take_br_rx().unwrap();
    let net_rx = channels.take_net_rx().unwrap();
    let stats_rx = channels.take_stats_rx().unwrap();
    let netbird_rx = channels.take_netbird_rx().unwrap();
    let idle_rx = channels.take_idle_rx().unwrap();
    let monitor_clone = monitor.clone();
    let br_tx = channels.br_tx();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        while let Ok(command) = media_rx.try_recv() {
            if let MediaCommand::Cmd(vol, mute, bt) = command {
                changed_progr.set(true);
                volume.set_value(vol as f64);
                changed_progr.set(false);
                volume.show_scale(!mute);
                volume.imp().button.set_active(mute);
                if mute {
                    volume.imp().button.add_css_class("muted");
                    volume.imp().second_icon.add_child_css_class("muted");
                    volume
                        .imp()
                        .main_icon
                        .set_icon_path("icons/volume-mute.svg");
                } else {
                    volume.imp().button.remove_css_class("muted");
                    volume.imp().second_icon.remove_child_css_class("muted");
                    let idx = ((volume.value() / 100.0) * (VOLUME_ICON_PATHS.len() - 1) as f64
                        + 0.5) as usize;
                    volume.imp().main_icon.set_icon_path(VOLUME_ICON_PATHS[idx]);
                }
                volume.imp().second_icon.set_visible(bt);
            }
        }
        while let Ok(message) = br_rx.try_recv() {
            if let BrightnessMessage::Value(mon, value, _) = message
                && monitor.eq(&mon)
            {
                brightness.set_value(value as f64);
                let idx =
                    ((value as f32 / 100.0) * (BRIGHTNESS_ICONS.len() - 1) as f32 + 0.5) as usize;
                brightness
                    .imp()
                    .main_icon
                    .set_font_icon(BRIGHTNESS_ICONS[idx]);
            }
        }
        while let Ok(state) = idle_rx.try_recv() {
            match state {
                IdleState::Dimmed => {
                    if !caffeine_button.has_css_class("idle") && monitor_valid {
                        let _ =
                            br_tx.send(BrightnessMessage::Value(monitor_clone.clone(), 20, true));
                        brightness.set_value(20.0);
                        println!("Dimmed: {monitor_clone}");
                    }
                    caffeine_button.add_css_class("idle");
                    caffeine_button.remove_css_class("inhibit");
                    caffeine_button.remove_css_class("on");
                }
                other => {
                    caffeine_button.remove_css_class("idle");
                    caffeine_button.remove_css_class("inhibit");
                    caffeine_button.remove_css_class("on");
                    if monitor_valid {
                        let _ = br_tx.send(BrightnessMessage::Revert(monitor_clone.clone()));
                    }
                    match other {
                        IdleState::Caffeine => caffeine_button.add_css_class("on"),
                        IdleState::Inhibited => caffeine_button.add_css_class("inhibit"),
                        IdleState::Active => {}
                        _ => {}
                    }
                }
            }
        }
        while let Ok(result) = net_rx.try_recv() {
            trace!(&LOGGER, "NetworkResult: {:?}", result);
            match result {
                NetworkState::ChangeNetwork(cfg) => {
                    match cfg {
                        Some(network) => {
                            match network.network_type {
                                NetworkType::Wifi => {
                                    //let idx = ((20.0 / 100.0) * (WIFI_ICON_PATHS.len() - 1) as f32 + 0.5) as usize;
                                    let idx = ((network.signal as f32 / 100.0)
                                        * (WIFI_ICON_PATHS.len() - 1) as f32
                                        + 0.5)
                                        as usize;
                                    network_icon.set_from_file(Some(WIFI_ICON_PATHS[idx]));
                                    network_label.set_label(&network.name);
                                    network_label.set_visible(true);
                                }
                                NetworkType::Wired => {
                                    network_icon.set_from_file(Some("icons/ethernet.svg"));
                                    network_label.set_visible(false);
                                }
                            }
                        }
                        None => {
                            network_icon.set_from_file(Some("icons/disconnected.svg"));
                            network_label.set_visible(false);
                        }
                    }
                }
                NetworkState::VpnState { connected, profile } if profile.as_str() == "wg" => {
                    if connected {
                        wg_button.add_css_class("on");
                    } else {
                        wg_button.remove_css_class("on");
                    }
                }
                _ => {}
            }
        }
        while let Ok(tailscale_state) = netbird_rx.try_recv() {
            match tailscale_state {
                json_socket::State::Disconnected => {
                    netbird_button.remove_css_class("on");
                    netbird_button.remove_css_class("starting");
                    netbird_button.remove_css_class("unknown");
                    netbird_button.remove_css_class("login");
                }
                json_socket::State::Connecting => {
                    netbird_button.remove_css_class("on");
                    netbird_button.add_css_class("starting");
                    netbird_button.remove_css_class("unknown");
                    netbird_button.remove_css_class("login");
                }
                json_socket::State::Connected => {
                    netbird_button.add_css_class("on");
                    netbird_button.remove_css_class("starting");
                    netbird_button.remove_css_class("unknown");
                    netbird_button.remove_css_class("login");
                }
                json_socket::State::NeedsLogin => {
                    netbird_button.remove_css_class("on");
                    netbird_button.remove_css_class("starting");
                    netbird_button.remove_css_class("unknown");
                    netbird_button.add_css_class("login");
                }
            }
        }
        if wg_button.has_css_class("on") || netbird_button.has_css_class("on") {
            vpn_button.add_css_class("on");
            vpn_button.remove_css_class("login");
        } else if wg_button.has_css_class("login") || netbird_button.has_css_class("login") {
            vpn_button.add_css_class("login");
            vpn_button.remove_css_class("on");
        } else {
            vpn_button.remove_css_class("on");
            vpn_button.remove_css_class("login");
        }
        while let Ok(stats) = stats_rx.try_recv() {
            cpu_label.set_label(&format!("{}%", stats.cpu));
            ram_label.set_label(&format!("{}%", stats.ram));
            for disk in stats.disks {
                if disk.0.eq("/") {
                    disk_label.set_label(&format!("{}%", disk.1));
                    break;
                }
            }

            if let Some(gpu) = stats.gpus.into_iter().next() {
                gpu_label.set_label(&format!("{}%", gpu.1));
            }

            set_battery_css_classes(&battery_widget, stats.battery_state, stats.battery_charge);
            let idx = ((stats.battery_charge as f32 / 100.0) * (BATTERY_ICONS.len() - 1) as f32
                + 0.5) as usize;
            let idx = idx.min(BATTERY_ICONS.len() - 1);
            battery_widget.set_title(&format!("{} {}%", BATTERY_ICONS[idx], stats.battery_charge));
            wattage.set_label(&format!("  {:.1}W", stats.battery_draw));
            remaining_time.set_label(&format!("  {}h", format_remaining(stats.battery_time)));
        }
        ControlFlow::Continue
    });
    utilities
}

fn format_remaining(ms: u64) -> String {
    let total_minutes = ms / 60_000;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;

    format!("{hours}:{minutes:02}")
}

fn set_battery_css_classes(battery_widget: &ExpandableBarItem, state: State, percentage: u8) {
    if state == State::Charging {
        battery_widget.add_css_class("charging");
    } else {
        battery_widget.remove_css_class("charging");
        battery_widget.remove_css_class("critical");
        battery_widget.remove_css_class("low");
        battery_widget.remove_css_class("medium");
        match percentage {
            0..=10 => {
                battery_widget.add_css_class("critical");
            }
            11..=20 => {
                battery_widget.add_css_class("low");
            }
            21..=50 => {
                battery_widget.add_css_class("medium");
            }
            51..=100 => {}
            101.. => {
                battery_widget.set_title("<invalid>");
            }
        }
    }
}
