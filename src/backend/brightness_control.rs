use crate::LOGGER;
use crate::backend::{BackendThread, Registrator};
use brightness::blocking::{Brightness, BrightnessDevice};
use crossbeam_channel::{Receiver, Sender};
use ddc_hi::Ddc;
use jmb_logger_rs::*;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Formatter};
use std::os::unix::prelude::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fs, thread};

pub enum DisplayBackend {
    Sysfs(BrightnessDevice),
    DdcCi(Box<ddc_hi::Display>),
}

impl Debug for DisplayBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplayBackend::Sysfs(dev) => {
                return dev.fmt(f);
            }
            DisplayBackend::DdcCi(displ) => {
                f.write_str(&displ.info.id).expect("Error");
            }
        }
        Ok(())
    }
}

fn get_sysfs_dev(id: &str) -> Option<BrightnessDevice> {
    brightness::blocking::brightness_devices()
        .find(|dev| {
            if let Ok(dev) = dev
                && let Ok(name) = dev.device_name()
                && name.eq(id)
            {
                true
            } else {
                false
            }
        })
        .and_then(|b| b.ok())
}

impl DisplayBackend {
    pub fn set_brightness(&mut self, percent: u8) {
        match self {
            DisplayBackend::Sysfs(dev) => {
                let _ = dev.set(percent as u32);
            }
            DisplayBackend::DdcCi(display) => {
                let _ = display.handle.set_vcp_feature(0x10, percent as u16);
            }
        }
    }

    pub fn get_brightness(&mut self) -> Option<u8> {
        match self {
            DisplayBackend::Sysfs(dev) => dev.get().map(|val| val as u8).ok(),
            DisplayBackend::DdcCi(display) => {
                if let Ok(val) = display.handle.get_vcp_feature(0x10) {
                    Some(val.value() as u8)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum BrightnessMessage {
    Init(Sender<BrightnessMessage>),
    Value(String, u8, bool),
    Revert(String),
    Request(String),
}

#[derive(Debug)]
struct DisplayData {
    backend: DisplayBackend,
    value: u8,
    saved_value: Option<u8>,
}

impl DisplayData {
    fn new(backend: DisplayBackend, value: u8) -> Self {
        Self {
            backend,
            value,
            saved_value: None,
        }
    }

    fn save(&mut self) -> Option<&u8> {
        self.saved_value = Some(self.value);
        self.saved_value.as_ref()
    }

    fn update(&mut self) -> bool {
        if let Some(value) = self.backend.get_brightness()
            && self.value != value
        {
            self.value = value;
            return true;
        }
        false
    }

    fn set_brightness(&mut self, percent: u8) {
        self.backend.set_brightness(percent);
        self.value = percent;
    }
}

pub struct BrightnessManager {
    display_cache: HashMap<String, DisplayData>,
    subscribers: Vec<Sender<BrightnessMessage>>,
}

impl BackendThread<BrightnessMessage, Sender<BrightnessMessage>> for BrightnessManager {
    fn subscribe(
        registrator: Registrator<BrightnessMessage>,
    ) -> (Receiver<BrightnessMessage>, Sender<BrightnessMessage>) {
        let (b2f_tx, b2f_rx) = crossbeam_channel::unbounded::<BrightnessMessage>();
        let _ = registrator.send(b2f_tx);
        while let Ok(message) = b2f_rx.recv() {
            return match message {
                BrightnessMessage::Init(sender) => (b2f_rx, sender),
                _ => continue,
            };
        }
        panic!("Channel closed unexpectedly, without returning anything")
    }

    fn subscribers(&self) -> &Vec<Sender<BrightnessMessage>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<BrightnessMessage>>) {
        let (f2b_tx, f2b_rx) = crossbeam_channel::unbounded::<BrightnessMessage>();
        let instance = Arc::new(Mutex::new(Self {
            display_cache: HashMap::new(),
            subscribers: Vec::new(),
        }));
        let instance_clone = instance.clone();
        thread::spawn(move || {
            while let Ok(tx) = registration_rx.recv() {
                if tx.send(BrightnessMessage::Init(f2b_tx.clone())).is_ok() {
                    debug!(&LOGGER, "Added Brightness Subscriber: {tx:?}");
                    instance_clone
                        .lock()
                        .expect("Couldn't get lock")
                        .subscribers
                        .push(tx);
                }
            }
        });
        let instance_clone = instance.clone();
        thread::spawn(move || {
            loop {
                // Ensure all Messages are handled in the correct order
                let mut operations: HashMap<String, Vec<Operation>> = HashMap::new();
                let mut requests: HashSet<String> = HashSet::new();
                if let Ok(msg) = f2b_rx.recv() {
                    Self::insert_message_operation(&mut operations, &mut requests, msg);
                    while let Ok(msg) = f2b_rx.try_recv() {
                        Self::insert_message_operation(&mut operations, &mut requests, msg);
                    }
                }

                let mut manager = instance_clone.lock().unwrap();
                for (id, operations) in operations {
                    let mut last = manager
                        .display_cache
                        .get(&id)
                        .and_then(|d| Some(d.value))
                        .unwrap_or(60);
                    let mut revert_flag = false;
                    for operation in operations {
                        match operation {
                            Operation::Set(val, save) => {
                                if save {
                                    manager.display_cache.get_mut(&id).and_then(|d| d.save());
                                }
                                last = val;
                                revert_flag = false;
                            }
                            Operation::Revert => {
                                last = manager
                                    .display_cache
                                    .get_mut(&id)
                                    .and_then(|d| d.saved_value.take())
                                    .unwrap_or(last);
                                revert_flag = true;
                            }
                        }
                    }
                    if revert_flag {
                        manager.publish(BrightnessMessage::Value(id.clone(), last, false));
                    }
                    manager.set_brightness(id, last);
                }
                for id in requests {
                    if let Some(val) = manager.get_brightness(id.clone()) {
                        manager.publish(BrightnessMessage::Value(id, val, false));
                    }
                }
            }
        });
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(10_000));
                let mut messages = vec![];

                for (monitor, data) in
                    &mut instance.lock().expect("Couldn't get lock").display_cache
                {
                    if data.update() {
                        messages.push(BrightnessMessage::Value(monitor.clone(), data.value, false));
                    }
                }
                for message in messages {
                    instance.lock().expect("Couldn't get lock").publish(message);
                }
            }
        });
    }
}

#[derive(Debug)]
enum Operation {
    Set(u8, bool),
    Revert,
}

impl BrightnessManager {
    pub fn set_brightness(&mut self, connector: String, percent: u8) {
        if !self.display_cache.contains_key(&connector) {
            self.register_display(connector.clone());
        }
        if let Some(display) = self.display_cache.get_mut(&connector) {
            display.set_brightness(percent);
        }
    }

    pub fn get_brightness(&mut self, connector: String) -> Option<u8> {
        if !self.display_cache.contains_key(&connector) {
            self.register_display(connector.clone());
        }
        if let Some(display) = self.display_cache.get_mut(&connector) {
            display.backend.get_brightness()
        } else {
            None
        }
    }

    fn register_display(&mut self, connector: String) {
        let Ok(dir) = fs::read_dir("/sys/class/backlight/") else {
            return;
        };
        for path in dir {
            let Ok(bl_path) = path else { continue };
            let Ok(dir) = fs::read_dir(bl_path.path()) else {
                continue;
            };
            for path in dir {
                let Ok(path) = path else { continue };
                if path.file_name().eq("device")
                    && let Ok(link) = fs::read_link(path.path())
                    && let Some(name) = link.file_name()
                    && name
                        .to_str()
                        .is_some_and(|name| name.eq(&format!("card1-{connector}")))
                {
                    debug!(&LOGGER, "Found sysfs display {connector}");
                    let path = bl_path.file_name();
                    let Some(name) = path.to_str() else { continue };
                    let Some(device) = get_sysfs_dev(name) else {
                        continue;
                    };
                    let mut backend = DisplayBackend::Sysfs(device);
                    let value = backend.get_brightness().unwrap_or(60);
                    self.display_cache
                        .insert(connector.clone(), DisplayData::new(backend, value));
                    return;
                }
            }
        }
        let res = Self::ddcutil_bus_for_connector(&connector);
        if let Ok(path) = res
            && let Ok(data) = fs::metadata(path)
        {
            let id = data.rdev();
            debug!(&LOGGER, "Found DDC/CI display {connector}");
            for display in ddc_hi::Display::enumerate() {
                if id.to_string().eq(&display.info.id) {
                    let mut backend = DisplayBackend::DdcCi(Box::new(display));
                    let value = backend.get_brightness().unwrap_or(60);
                    self.display_cache
                        .insert(connector.clone(), DisplayData::new(backend, value));
                    break;
                }
            }
        }
    }

    fn ddcutil_bus_for_connector(connector: &str) -> Result<PathBuf, Box<dyn Error>> {
        let output = Command::new("ddcutil")
            .args(["detect", "--verbose"])
            .output()?;

        if !output.status.success() {
            return Err(std::io::Error::other("ddcutil detect --verbose failed").into());
        }

        // Split the output into display blocks.
        let mut blocks: Vec<Vec<&str>> = Vec::new();

        for line in std::str::from_utf8(&output.stdout)?.lines() {
            if line.starts_with("Display ") || line.starts_with("Invalid display") {
                blocks.push(vec![line]);
            } else if let Some(block) = blocks.last_mut() {
                block.push(line);
            }
        }

        for block in blocks {
            // Skip displays that ddcutil identifies as invalid.
            if block[0].starts_with("Invalid display") {
                continue;
            }

            let mut bus: Option<PathBuf> = None;
            let mut drm_connector: Option<String> = None;

            for line in block.iter().skip(1) {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };

                // Accept both "DRM connector" and "DRM_connector".
                let key = key.trim().replace('_', " ");
                let value = value.trim();

                if key.eq_ignore_ascii_case("I2C bus") {
                    bus = Some(PathBuf::from(value));
                } else if key.eq_ignore_ascii_case("DRM connector") {
                    // Convert "card1-HDMI-A-1" to "HDMI-A-1".
                    drm_connector = Some(
                        value
                            .strip_prefix("card")
                            .and_then(|s| s.split_once('-'))
                            .map(|(_, name)| name)
                            .unwrap_or(value)
                            .to_owned(),
                    );
                }
            }

            if drm_connector.as_deref() == Some(connector)
                && let Some(bus) = bus
            {
                return Ok(bus);
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No DDC-capable display found for {connector}"),
        )
        .into())
    }

    fn insert_message_operation(
        operations: &mut HashMap<String, Vec<Operation>>,
        requests: &mut HashSet<String>,
        msg: BrightnessMessage,
    ) {
        match msg {
            BrightnessMessage::Value(id, val, save) => {
                if !operations.contains_key(&id) {
                    operations.insert(id.clone(), Vec::new());
                }
                let ops = operations.get_mut(&id);
                if let Some(ops) = ops {
                    ops.push(Operation::Set(val, save));
                }
            }
            BrightnessMessage::Revert(id) => {
                if !operations.contains_key(&id) {
                    operations.insert(id.clone(), Vec::new());
                }
                let ops = operations.get_mut(&id);
                if let Some(ops) = ops {
                    ops.push(Operation::Revert);
                }
            }
            BrightnessMessage::Request(id) => {
                requests.insert(id);
            }
            _ => {}
        }
    }
}
