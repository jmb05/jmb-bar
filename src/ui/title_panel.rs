use crate::backend::window_manager_hypr::HyprlandComms;
use glib::prelude::CastNone;
use gtk::pango::EllipsizeMode;
use gtk::prelude::{ButtonExt, GestureSingleExt, WidgetExt};
use gtk::{Align, Button, GestureClick, Label};
use mpris::{PlaybackStatus, Player, PlayerFinder};

pub fn build() -> Button {
    let title_label = Label::new(None);
    title_label.set_ellipsize(EllipsizeMode::End);
    title_label.set_max_width_chars(50);

    let title = Button::new();
    title.set_vexpand(true);
    title.set_valign(Align::Fill);
    title.set_child(Some(&title_label));
    title.set_visible(false);
    title.add_css_class("title");
    title.connect_clicked(|button| {
        if button.has_css_class("media") {
            if let Ok(player_finder) = PlayerFinder::new() {
                if let Ok(player) = player_finder.find_active() {
                    let _ = player.play_pause();
                }
            }
        }
    });

    let right_click = GestureClick::new();
    right_click.set_button(3);
    right_click.connect_pressed(|_gesture, _n_press, _, _| {
        if let Ok(player_finder) = PlayerFinder::new() {
            if let Ok(player) = player_finder.find_active() {
                let _ = player.stop();
            }
        }
    });
    title.add_controller(right_click);
    title
}

pub fn manage_title(button_opt: &Option<Button>, player_opt: &Option<Player>) {
    let mut handled = false;
    if let Some(button) = button_opt {
        if let Some(player) = player_opt {
            if let Ok(status) = player.get_playback_status() {
                if let Ok(metadata) = player.get_metadata() {
                    match status {
                        PlaybackStatus::Playing => {
                            button.set_visible(true);
                            button.child()
                                .and_downcast::<Label>()
                                .unwrap()
                                .set_label(&format!(" {}", metadata.title().unwrap_or("")));
                            button.add_css_class("media");
                            button.add_css_class("playing");
                            handled = true;
                        }
                        PlaybackStatus::Paused => {
                            button.set_visible(true);
                            button.child()
                                .and_downcast::<Label>()
                                .unwrap()
                                .set_label(&format!(" {}", metadata.title().unwrap_or("")));
                            button.add_css_class("media");
                            button.remove_css_class("playing");
                            handled = true;
                        }
                        PlaybackStatus::Stopped => {},
                    }
                }
            }
        }
        if !handled {
            button.set_visible(true);
            button.remove_css_class("media");
            button.remove_css_class("playing");
            set_title(&HyprlandComms::get_current_title(), button_opt);
        }
    }
}

pub fn set_title(title: &str, title_button: &Option<Button>) {
    if let Some(button) = title_button {
        if title.is_empty() {
            button.set_visible(false);
        } else {
            button.set_visible(true);
        }
        button.child()
            .and_downcast::<Label>()
            .unwrap()
            .set_label(&HyprlandComms::get_current_title());
    }
}