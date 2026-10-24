use glib::ParamSpec;
use glib::object::IsA;
use glib::prelude::ObjectExt;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{Adjustment, Button, Label, Scale, ToggleButton, Widget};

mod imp_label_entry {
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::box_::BoxImpl;
    use gtk::subclass::prelude::*;
    use gtk::{Entry, Label, Orientation};

    #[derive(Default)]
    pub struct LabelEntry {
        pub label: std::cell::OnceCell<Label>,
        pub entry: std::cell::OnceCell<Entry>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LabelEntry {
        const NAME: &'static str = "EntryLabel";
        type Type = super::LabelEntry;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for LabelEntry {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(Orientation::Horizontal);
            obj.set_spacing(4);

            let label = Label::new(None);
            let entry = Entry::new();
            obj.append(&label);
            obj.append(&entry);

            self.label.set(label.clone()).unwrap();
            self.entry.set(entry.clone()).unwrap();
        }
    }

    impl WidgetImpl for LabelEntry {}
    impl BoxImpl for LabelEntry {}
}

mod imp_icon_label {
    use crate::ui::widgets::Icon;
    use glib::Properties;
    use glib::property::PropertySet;
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::box_::BoxImpl;
    use gtk::subclass::prelude::*;
    use gtk::{Label, Orientation};
    use std::cell::RefCell;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::IconLabel)]
    pub struct IconLabel {
        pub icon: std::cell::OnceCell<Icon>,
        pub label: std::cell::OnceCell<Label>,
        #[property(get, set = Self::set_icon_path, name = "icon-path")]
        icon_path: RefCell<String>,
        #[property(get, set = Self::set_icon_name, name = "icon-name")]
        icon_name: RefCell<String>,
        #[property(get, set = Self::set_font_icon, name = "font-icon")]
        font_icon: RefCell<String>,
        #[property(get, set = Self::set_label, name = "label")]
        label_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconLabel {
        const NAME: &'static str = "JBIconLabel";
        type Type = super::IconLabel;
        type ParentType = gtk::Box;
    }

    #[glib::derived_properties]
    impl ObjectImpl for IconLabel {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(Orientation::Horizontal);
            obj.set_spacing(4);

            let icon = Icon::new();
            obj.append(&icon);

            let label = Label::new(None);
            obj.append(&label);

            self.icon.set(icon.clone()).unwrap();
            self.label.set(label.clone()).unwrap();
        }
    }

    impl WidgetImpl for IconLabel {}
    impl BoxImpl for IconLabel {}

    impl IconLabel {
        fn set_icon_path(&self, path: String) {
            self.icon_path.set(path.clone());
            self.icon.get().unwrap().set_icon_path(path);
        }

        pub fn set_icon_name(&self, name: String) {
            self.icon_name.set(name.clone());
            self.icon.get().unwrap().set_icon_name(name);
        }

        pub fn set_font_icon(&self, icon: &str) {
            self.font_icon.set(icon.to_string());
            self.icon.get().unwrap().set_font_icon(icon);
        }

        pub fn set_label(&self, text: &str) {
            self.label_text.set(text.to_string());
            self.label.get().unwrap().set_text(text);
        }
    }
}

mod imp_icon {
    use glib::Properties;
    use glib::object::ObjectExt;
    use glib::property::PropertySet;
    use glib::subclass::InitializingObject;
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::prelude::*;
    use gtk::prelude::WidgetExt;
    use gtk::subclass::box_::BoxImpl;
    use gtk::subclass::prelude::*;
    use gtk::{CompositeTemplate, Image, Label, TemplateChild};
    use std::cell::{Cell, RefCell};

    #[derive(CompositeTemplate, Default, Properties)]
    #[properties(wrapper_type = super::Icon)]
    #[template(resource = "/net/jmb19905/jmb-bar/icon.ui")]
    pub struct Icon {
        #[template_child]
        pub image: TemplateChild<Image>,
        #[template_child]
        pub label: TemplateChild<Label>,
        #[property(get, set = Self::add_child_css_class, name = "child-css-class")]
        child_css_class: RefCell<String>,
        #[property(get, set = Self::set_icon_size, minimum = 1, maximum = 255, default = 16, name = "pixel-size")]
        pixel_size: Cell<i32>,
        #[property(get, set = Self::set_icon_path, name = "icon-path")]
        icon_path: RefCell<String>,
        #[property(get, set = Self::set_icon_name, name = "icon-name")]
        icon_name: RefCell<String>,
        #[property(get, set = Self::set_font_icon, name = "font-icon")]
        font_icon: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Icon {
        const NAME: &'static str = "JBIcon";
        type Type = super::Icon;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Icon {}
    impl WidgetImpl for Icon {}
    impl BoxImpl for Icon {}

    impl Icon {
        fn add_child_css_class(&self, class: &str) {
            self.child_css_class.set(class.to_string());
            if self.label.is_bound() {
                self.label.add_css_class(class);
            }
            if self.image.is_bound() {
                self.image.add_css_class(class);
            }
        }

        fn set_icon_size(&self, size: i32) {
            self.pixel_size.set(size);

            if self.image.is_bound() {
                self.image.set_pixel_size(size);
            }
        }

        fn set_icon_path(&self, path: String) {
            self.image.set_from_file(Some(path));
            self.label.set_visible(false);
            self.image.set_visible(true);
        }

        pub fn set_icon_name(&self, name: String) {
            self.image.set_icon_name(Some(&name));
            self.label.set_visible(false);
            self.image.set_visible(true);
        }

        pub fn set_font_icon(&self, icon: &str) {
            self.label.set_label(icon);
            self.label.set_visible(true);
            self.image.set_visible(false);
        }
    }
}

mod imp_icon_slider {
    use glib::subclass::InitializingObject;
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::prelude::ObjectSubclass;
    use gtk::subclass::box_::BoxImpl;
    use gtk::subclass::prelude::*;
    use gtk::{CompositeTemplate, TemplateChild, ToggleButton};

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/net/jmb19905/jmb-bar/icon_slider.ui")]
    pub struct IconSlider {
        #[template_child]
        pub button: TemplateChild<ToggleButton>,
        #[template_child]
        pub main_icon: TemplateChild<super::Icon>,
        #[template_child]
        pub second_icon: TemplateChild<super::Icon>,
        #[template_child]
        pub slider: TemplateChild<gtk::Scale>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconSlider {
        const NAME: &'static str = "JBIconSlider";
        type Type = super::IconSlider;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for IconSlider {}

    impl WidgetImpl for IconSlider {}

    impl BoxImpl for IconSlider {}
}

mod imp_bar_item {
    use glib::Properties;
    use glib::object::ObjectExt;
    use glib::property::PropertySet;
    use glib::subclass::InitializingObject;
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::prelude::*;
    use gtk::prelude::{ButtonExt, WidgetExt};
    use gtk::subclass::box_::BoxImpl;
    use gtk::subclass::prelude::*;
    use gtk::{Button, CompositeTemplate, EventControllerMotion, Revealer, TemplateChild, Widget};
    use std::cell::RefCell;

    #[derive(CompositeTemplate, Default, Properties)]
    #[properties(wrapper_type = super::ExpandableBarItem)]
    #[template(resource = "/net/jmb19905/jmb-bar/expandable_bar_item.ui")]
    pub struct ExpandableBarItem {
        #[template_child]
        pub main_button: TemplateChild<Button>,
        #[template_child]
        pub revealer: TemplateChild<Revealer>,
        #[property(get, set = Self::set_main_child, name = "main-child")]
        pub main_child: RefCell<Option<Widget>>,
        #[property(get, set = Self::set_second_child, name = "second-child")]
        pub second_child: RefCell<Option<Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ExpandableBarItem {
        const NAME: &'static str = "JBExpandableBarItem";
        type Type = super::ExpandableBarItem;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ExpandableBarItem {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            let motion = EventControllerMotion::new();

            let revealer_enter = self.revealer.clone();
            let button_enter = self.main_button.clone();

            motion.connect_enter(move |_, _, _| {
                revealer_enter.set_reveal_child(true);
                button_enter.add_css_class("hovered");
            });

            let revealer_leave = self.revealer.clone();
            let button_leave = self.main_button.clone();

            motion.connect_leave(move |_| {
                revealer_leave.set_reveal_child(false);
                button_leave.remove_css_class("hovered");
            });

            obj.add_controller(motion);
        }
    }

    impl WidgetImpl for ExpandableBarItem {}

    impl BoxImpl for ExpandableBarItem {}

    impl ExpandableBarItem {
        fn set_main_child(&self, widget: Option<&Widget>) {
            self.main_child.set(widget.cloned());
            self.main_button.set_child(widget);
        }

        fn set_second_child(&self, widget: Option<&Widget>) {
            self.second_child.set(widget.cloned());
            self.revealer.set_child(widget);
        }
    }
}

mod imp_wifi_widget {
    use glib::subclass::InitializingObject;
    use glib::subclass::prelude::ObjectSubclass;
    use gtk::subclass::prelude::*;
    use gtk::{Button, CompositeTemplate, Label, PasswordEntry, Spinner, Stack, TemplateChild};

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/net/jmb19905/jmb-bar/wifi_widget.ui")]
    pub struct WifiWidget {
        #[template_child]
        pub wifi_icon: TemplateChild<super::Icon>,
        #[template_child]
        pub spinner: TemplateChild<Spinner>,
        #[template_child]
        pub stack: TemplateChild<Stack>,
        #[template_child]
        pub ssid: TemplateChild<Label>,
        #[template_child]
        pub password: TemplateChild<PasswordEntry>,
        #[template_child]
        pub known: TemplateChild<super::Icon>,
        #[template_child]
        pub secure: TemplateChild<super::Icon>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WifiWidget {
        const NAME: &'static str = "JBWifiWidget";
        type Type = super::WifiWidget;
        type ParentType = Button;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for WifiWidget {}
    impl WidgetImpl for WifiWidget {}
    impl ButtonImpl for WifiWidget {}
}

glib::wrapper! {
    pub struct LabelEntry(ObjectSubclass<imp_label_entry::LabelEntry>)
        @extends gtk::Box, Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl LabelEntry {
    pub fn new(title: &str) -> Self {
        let obj: Self = glib::Object::new();

        if let Some(label) = obj.clone().downcast::<LabelEntry>().unwrap().label() {
            label.set_label(title);
        }

        obj
    }

    pub fn label(&self) -> Option<Label> {
        let imp = self.imp();

        if let Some(label) = imp.label.get() {
            return Some(label.clone());
        }
        None
    }
}

glib::wrapper! {
    pub struct IconLabel(ObjectSubclass<imp_icon_label::IconLabel>)
        @extends gtk::Box, Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl IconLabel {
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj
    }
}

glib::wrapper! {
    pub struct Icon(ObjectSubclass<imp_icon::Icon>)
    @extends gtk::Box, Widget,
    @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Icon {
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj
    }
    pub fn from_font_icon(icon: &str) -> Self {
        let obj = Self::new();
        obj.set_font_icon(icon);
        obj
    }

    pub fn from_icon_path(
        path: Option<impl AsRef<std::path::Path> + std::borrow::Borrow<str>>,
    ) -> Self {
        let obj = Self::new();
        if let Some(path) = path {
            obj.set_icon_path(path);
        }
        obj
    }

    pub fn from_icon_name(name: &str) -> Self {
        let obj = Self::new();
        obj.set_icon_name(name);
        obj
    }

    pub fn add_child_css_class(&self, class: &str) {
        self.imp().image.add_css_class(class);
        self.imp().label.add_css_class(class);
    }

    pub fn remove_child_css_class(&self, class: &str) {
        self.imp().image.remove_css_class(class);
        self.imp().label.remove_css_class(class);
    }
}

glib::wrapper! {
    pub struct IconSlider(ObjectSubclass<imp_icon_slider::IconSlider>)
    @extends gtk::Box, Widget,
    @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl IconSlider {
    pub fn new(adjustment: Option<Adjustment>) -> Self {
        let obj: Self = glib::Object::new();
        if let Some(adj) = adjustment {
            obj.imp().slider.set_adjustment(&adj);
        }
        obj
    }

    pub fn show_second_icon(&self, val: bool) {
        self.imp().second_icon.set_visible(val);
    }

    pub fn connect_value_changed<A>(&self, action: A)
    where
        A: Fn(&IconSlider, &ToggleButton, &Scale) + 'static,
    {
        let clone = self.clone();
        self.imp().slider.connect_value_changed(move |scale| {
            action(&clone, &clone.imp().button, scale);
        });
    }

    pub fn connect_clicked<A>(&self, action: A)
    where
        A: Fn(&IconSlider, &ToggleButton, &Scale) + 'static,
    {
        let clone = self.clone();
        self.imp().button.connect_clicked(move |button| {
            action(&clone, button, &clone.imp().slider);
        });
    }

    pub fn value(&self) -> f64 {
        self.imp().slider.value()
    }

    pub fn set_value(&self, val: f64) {
        self.imp().slider.set_value(val);
    }

    pub fn show_scale(&self, val: bool) {
        self.imp().slider.set_visible(val);
    }
}

glib::wrapper! {
    pub struct ExpandableBarItem(ObjectSubclass<imp_bar_item::ExpandableBarItem>)
    @extends gtk::Box, Widget,
    @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl ExpandableBarItem {
    pub fn new(title: &str) -> Self {
        let obj: Self = glib::Object::new();

        obj.set_title(title);

        obj
    }

    pub fn set_title(&self, title: &str) {
        self.imp().main_button.set_label(title);
    }

    pub fn set_icon(&self, icon_name: &str) {
        self.imp().main_button.set_label(icon_name);
    }

    pub fn set_button_child(&self, child: Option<&impl IsA<Widget>>) {
        self.imp().main_button.set_child(child);
    }

    pub fn connect_clicked<F>(&self, action: F)
    where
        F: Fn(&Button) + 'static,
    {
        self.imp().main_button.connect_clicked(action);
    }
}

glib::wrapper! {
    pub struct WifiWidget(ObjectSubclass<imp_wifi_widget::WifiWidget>)
        @extends Button, Widget,
        @implements gtk::Actionable, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WifiWidget {
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj
    }

    pub fn set_ssid(&self, ssid: &str) {
        self.imp().ssid.set_label(ssid);
    }

    pub fn ssid(&self) -> glib::GString {
        self.imp().ssid.get().text()
    }

    pub fn set_secured(&self, secured: bool) {
        if secured {
            let _ = &self.imp().secure.set_icon_name("channel-secure-symbolic");
        }
    }

    pub fn set_known(&self, known: bool) {
        if known {
            let _ = &self.imp().known.set_icon_name("user-bookmarks-symbolic");
        }
    }

    pub fn grab_focus_password_entry(&self) -> bool {
        self.imp().password.grab_focus()
    }

    pub fn stop_spinner(&self) {
        self.imp().spinner.stop();
    }

    pub fn start_spinner(&self) {
        self.imp().spinner.start();
    }

    pub fn set_stack_child(&self, name: &str) {
        self.imp().stack.set_visible_child_name(name);
    }

    pub fn show_password_entry(&self) {
        self.set_stack_child("pass");
    }

    pub fn show_ssid(&self) {
        self.set_stack_child("ssid");
    }

    pub fn set_wifi_icon_from_file(&self, icon_path: &str) {
        self.imp().wifi_icon.set_icon_path(icon_path);
    }

    pub fn set_wifi_icon_from_name(&self, icon: &str) {
        self.imp().wifi_icon.set_icon_name(icon);
    }

    pub fn connect_activate_pass<F>(&self, action: F)
    where
        F: Fn(&gtk::PasswordEntry) + 'static,
    {
        self.imp().password.connect_activate(action);
    }

    pub fn connect_has_focus_pass<F>(&self, action: F)
    where
        F: Fn(&gtk::PasswordEntry, &ParamSpec) + 'static,
    {
        self.imp()
            .password
            .connect_notify_local(Some("has-focus"), action);
    }
}
