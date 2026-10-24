use crate::backend::BackendChannels;
use crate::backend::network::{
    Auth, AuthType, ConnectError, NetworkCommand, NetworkInfo, NetworkState,
};
use crate::ui::widgets::{LabelEntry, WifiWidget};
use glib::prelude::Cast;
use glib::{ControlFlow, clone};
use gtk::gdk::Display;
use gtk::prelude::{
    BoxExt, ButtonExt, DisplayExt, EditableExt, GtkWindowExt, ListModelExt, WidgetExt,
};
use gtk::{
    Application, ApplicationWindow, Button, Image, Label, Orientation, PolicyType, ScrolledWindow,
    Stack, gdk,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

const WIFI_ICONS: [&str; 4] = [
    "xsi-network-wireless-signal-none-symbolic",
    "xsi-network-wireless-signal-weak-symbolic",
    "xsi-network-wireless-signal-ok-symbolic",
    "xsi-network-wireless-signal-good-symbolic",
];
const _WIFI_ICONS_SECURED: [&str; 4] = [
    "xsi-network-wireless-signal-none-secure-symbolic",
    "xsi-network-wireless-signal-weak-secure-symbolic",
    "xsi-network-wireless-signal-ok-secure-symbolic",
    "xsi-network-wireless-signal-good-secure-symbolic",
];

const ANIMATION_DURATION: u64 = 200;

#[derive(Clone)]
pub struct ActionCentre {
    pub window: ApplicationWindow,
    is_animating: Rc<Cell<bool>>,
}

impl ActionCentre {
    pub fn create_action_centre(app: Application, mut channels: BackendChannels) -> Self {
        let window = ApplicationWindow::builder()
            .application(&app)
            .title("JmbBar")
            .default_width(400)
            .default_height(1080)
            .css_classes(vec!["action-centre"])
            .build();

        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Right, true);
        window.set_margin(Edge::Top, 20);
        window.set_margin(Edge::Right, 20);
        window.set_margin(Edge::Bottom, 20);
        window.set_keyboard_mode(KeyboardMode::OnDemand);

        let instance = Self {
            window: window.clone(),
            is_animating: Rc::new(Cell::new(false)),
        };

        let scrolled = ScrolledWindow::new();
        scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
        scrolled.set_vexpand(true);
        let content = gtk::Box::new(Orientation::Vertical, 6);

        let add_net_stack = Stack::new();
        add_net_stack.set_vhomogeneous(false);

        let add_network_button = Button::new();
        add_network_button.add_css_class("wifi-button");

        let add_net_box = gtk::Box::new(Orientation::Horizontal, 4);
        let add_icon = Image::from_file("icons/plus-symbolic.svg");
        add_icon.add_css_class("list-add-icon");
        add_icon.add_css_class("icon");
        add_icon.set_pixel_size(24);
        add_net_box.append(&add_icon);
        add_net_stack.add_named(&add_network_button, Some("button"));

        let add_net_input_box = gtk::Box::new(Orientation::Vertical, 4);
        let ssid = LabelEntry::new("SSID:");
        add_net_input_box.append(&ssid);
        let pw = LabelEntry::new("Passphrase:");
        add_net_input_box.append(&pw);
        add_net_stack.add_named(&add_net_input_box, Some("input"));

        add_net_stack.set_visible_child_name("button");

        add_network_button.connect_clicked(clone!(
            #[strong]
            add_net_stack,
            move |_button| {
                add_net_stack.set_visible_child_name("input");
            }
        ));

        let label = Label::new(Some("Add Network..."));
        add_net_box.append(&label);
        add_network_button.set_child(Some(&add_net_box));

        content.append(&add_net_stack);

        scrolled.set_child(Some(&content));

        window.set_child(Some(&scrolled));

        let instance_clone = instance.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                instance_clone.hide_action_centre();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key_controller);

        let wifi_networks: Rc<RefCell<HashMap<String, WifiWidget>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let wifi_networks_clone = wifi_networks.clone();
        window.connect_focus_widget_notify(move |window| {
            Self::focus_callback(&wifi_networks_clone, window)
        });

        let net_rx = channels.take_net_rx().unwrap();
        let window_clone = window.clone();
        let wifi_networks_clone = wifi_networks.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            while let Ok(message) = net_rx.try_recv() {
                match message {
                    NetworkState::ChangeNetwork(Some(info)) => {
                        for (name, widget) in wifi_networks_clone.borrow().iter() {
                            if name.eq(&info.name) {
                                widget.add_css_class("connected");
                            } else {
                                widget.remove_css_class("connected");
                            }
                        }
                    }
                    NetworkState::Networks(networks) => {
                        for network in &networks {
                            let borrowed_wifi_networks_clone = wifi_networks_clone.borrow_mut();
                            let widget = borrowed_wifi_networks_clone.get(&network.name);
                            if let Some(widget) = widget {
                                let idx = ((network.signal as f32 / 100.0)
                                    * (WIFI_ICONS.len() - 1) as f32
                                    + 0.5) as usize;
                                widget.set_wifi_icon_from_name(WIFI_ICONS[idx]);
                            } else {
                                let widget = Self::create_wifi_widget(
                                    &channels,
                                    network,
                                    borrowed_wifi_networks_clone,
                                );
                                content.append(&widget);
                            }
                        }
                        let mut removal: Vec<String> = Vec::new();
                        for (ssid, widget) in wifi_networks_clone.borrow().iter() {
                            let network = &networks.iter().find(|n| n.name.eq(ssid));
                            if network.is_none() {
                                content.remove(widget);
                                removal.push(ssid.clone());
                            }
                        }
                        for ssid in removal {
                            wifi_networks_clone.borrow_mut().remove(&ssid);
                        }
                    }
                    NetworkState::Auth(ssid, auth) => {
                        if let Some(widget) = Self::find_focused_widget(window_clone.as_ref()) {
                            match auth {
                                AuthType::Password => {
                                    if widget.ssid().eq(&ssid) {
                                        widget.show_password_entry();
                                        widget.stop_spinner();
                                        widget.grab_focus_password_entry();
                                    }
                                }
                            }
                        }
                    }
                    NetworkState::ConnectFail(ssid, err) => {
                        let mut networks = wifi_networks_clone.borrow_mut();
                        if let Some(widget) = networks.get(&ssid) {
                            widget.stop_spinner();
                            match err {
                                ConnectError::AuthFailed => widget.show_password_entry(),
                                ConnectError::NotFound => {
                                    content.remove(widget);
                                    networks.remove(&ssid);
                                }
                                ConnectError::Unknown => {}
                            }
                        }
                    }
                    NetworkState::ConnectSuccess(ssid) => {
                        if let Some(widget) = wifi_networks_clone.borrow().get(&ssid) {
                            widget.stop_spinner();
                        }
                    }
                    _ => {}
                }
            }
            ControlFlow::Continue
        });

        instance
    }

    fn create_wifi_widget(
        channels: &BackendChannels,
        network: &NetworkInfo,
        mut borrowed_wifi_networks_clone: RefMut<HashMap<String, WifiWidget>>,
    ) -> WifiWidget {
        let ssid = network.name.clone();
        let widget = WifiWidget::new();
        widget.set_ssid(&ssid);
        widget.add_css_class("wifi-button");
        widget.set_secured(network.secured.unwrap());
        widget.set_known(network.known);
        let idx = ((network.signal as f32 / 100.0) * (WIFI_ICONS.len() - 1) as f32 + 0.5) as usize;
        widget.set_wifi_icon_from_name(WIFI_ICONS[idx]);
        borrowed_wifi_networks_clone.insert(ssid.clone(), widget.clone());
        let net_tx = channels.net_tx();
        widget.connect_clicked(clone!(
            #[strong]
            net_tx,
            #[strong]
            ssid,
            move |widget| {
                let _ = net_tx.send(NetworkCommand::Connect(ssid.clone(), Auth::Open));
                widget.start_spinner();
            }
        ));
        widget.connect_activate_pass(clone!(
            #[strong]
            net_tx,
            #[weak]
            widget,
            move |entry| {
                let text = entry.text().to_string();
                if !text.is_empty() {
                    let _ = net_tx.send(NetworkCommand::Connect(
                        ssid.clone(),
                        Auth::Password(entry.text().to_string()),
                    ));
                    widget.start_spinner();
                    widget.show_ssid();
                }
            }
        ));
        widget
    }

    fn focus_callback(
        wifi_networks: &Rc<RefCell<HashMap<String, WifiWidget>>>,
        window: &ApplicationWindow,
    ) {
        for widget in wifi_networks.clone().borrow().values() {
            let focused_inside = window.focus().is_some_and(|mut focus| {
                loop {
                    if focus == *widget {
                        break true;
                    }

                    match focus.parent() {
                        Some(parent) => focus = parent,
                        None => break false,
                    }
                }
            });
            if !focused_inside {
                widget.show_ssid();
            }
        }
    }

    fn _has_focus_within(window: &ApplicationWindow) -> bool {
        let Some(mut widget) = window.focus() else {
            return false;
        };

        loop {
            if widget == *window {
                return true;
            }

            match widget.parent() {
                Some(parent) => widget = parent,
                None => return false,
            }
        }
    }

    fn find_focused_widget(window: &gtk::Window) -> Option<WifiWidget> {
        let mut widget = window.focus()?;

        loop {
            if let Ok(widget) = widget.clone().downcast::<WifiWidget>() {
                return Some(widget);
            }

            widget = widget.parent()?;
        }
    }

    pub fn show_action_centre(&self, monitor_idx: u32, display: &Display) {
        let window = &self.window;
        let monitors = display.monitors();
        let Some(monitor) = monitors.item(monitor_idx) else {
            return;
        };
        let Ok(monitor) = monitor.downcast::<gdk::Monitor>() else {
            return;
        };
        window.set_monitor(Some(&monitor));
        if !window.is_visible() {
            self.animate_margin(-window.width() - 20, 20, ANIMATION_DURATION, || {});
            window.present();
        }
    }

    pub fn hide_action_centre(&self) {
        let clone = self.clone();
        self.animate_margin(
            20,
            -clone.window.width() - 20,
            ANIMATION_DURATION,
            move || clone.window.set_visible(false),
        );
    }

    pub fn animate_margin(
        &self,
        from: i32,
        to: i32,
        duration_ms: u64,
        on_finished: impl Fn() + 'static,
    ) {
        let window = self.window.clone();
        let flag = self.is_animating.clone();
        let start = Instant::now();
        let duration = Duration::from_millis(duration_ms);

        if flag.get() {
            return;
        }

        glib::timeout_add_local(Duration::from_millis(10), move || {
            flag.set(true);
            let elapsed = start.elapsed();
            let t = (elapsed.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0);
            // Ease-out cubic.
            let eased = 1.0 - (1.0 - t).powi(3);

            let margin = from as f64 + (to - from) as f64 * eased;

            window.set_margin(Edge::Right, margin.round() as i32);

            if t >= 1.0 {
                on_finished();
                flag.set(false);
                ControlFlow::Break
            } else {
                ControlFlow::Continue
            }
        });
    }
}
