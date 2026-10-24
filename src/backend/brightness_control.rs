use crate::LOGGER;
use crate::backend::{BackendThread, Registrator};
use brightness::blocking::{Brightness, BrightnessDevice};
use crossbeam_channel::{Receiver, Sender};
use ddc_hi::Ddc;
use jmb_logger_rs::*;
use std::collections::HashMap;
use std::error::Error;
use std::os::unix::prelude::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fs, thread};

pub enum DisplayBackend {
    Sysfs(BrightnessDevice),
    DdcCi(ddc_hi::Display),
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
            DisplayBackend::Sysfs(dev) => dev.get().and_then(|val| Ok(val as u8)).ok(),
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
    Value(String, u8),
    Request(String),
}

struct DisplayData(DisplayBackend, u8);

impl DisplayData {
    fn update(&mut self) -> bool {
        if let Some(value) = self.0.get_brightness() {
            if self.1 != value {
                self.1 = value;
                return true;
            }
        }
        false
    }

    fn set_brightness(&mut self, percent: u8) {
        self.0.set_brightness(percent);
        self.1 = percent;
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
        std::thread::spawn(move || {
            while let Ok(tx) = registration_rx.recv() {
                if tx.send(BrightnessMessage::Init(f2b_tx.clone())).is_ok() {
                    trace!(&LOGGER, "Added Brightness Subscriber: {tx:?}");
                    instance_clone
                        .lock()
                        .expect("Couldn't get lock")
                        .subscribers
                        .push(tx);
                }
            }
        });
        let instance_clone = instance.clone();
        std::thread::spawn(move || {
            while let Ok(mut msg) = f2b_rx.recv() {
                while let Ok(newer) = f2b_rx.try_recv() {
                    msg = newer;
                }
                match msg {
                    BrightnessMessage::Value(id, val) => {
                        instance_clone
                            .lock()
                            .expect("Couldn't get lock")
                            .set_brightness(id, val);
                    }
                    BrightnessMessage::Request(id) => {
                        instance_clone
                            .lock()
                            .expect("Couldn't get lock")
                            .register_display(id.clone());
                        if let Some(val) = instance_clone
                            .lock()
                            .expect("Couldn't get lock")
                            .display_cache
                            .get(&id)
                            .and_then(|v| Some(v.1))
                        {
                            instance_clone
                                .lock()
                                .expect("Couldn't get lock")
                                .publish(BrightnessMessage::Value(id, val));
                        }
                    }
                    _ => {}
                }
            }
        });
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(10_000));
                let mut messages = vec![];

                for monitor in &mut instance.lock().expect("Couldn't get lock").display_cache {
                    if monitor.1.update() {
                        messages.push(BrightnessMessage::Value(monitor.0.clone(), monitor.1.1));
                    }
                }
                for message in messages {
                    instance.lock().expect("Couldn't get lock").publish(message);
                }
            }
        });
    }
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

    pub fn _get_brightness(&mut self, connector: String) -> Option<u8> {
        if !self.display_cache.contains_key(&connector) {
            self.register_display(connector.clone());
        }
        if let Some(display) = self.display_cache.get_mut(&connector) {
            display.0.get_brightness()
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
                {
                    if name
                        .to_str()
                        .is_some_and(|name| name.eq(&format!("card1-{connector}")))
                    {
                        println!("Found sysfs display {connector}");
                        let path = bl_path.file_name();
                        let Some(name) = path.to_str() else { continue };
                        let Some(device) = get_sysfs_dev(name) else {
                            continue;
                        };
                        let mut backend = DisplayBackend::Sysfs(device);
                        let value = backend.get_brightness().unwrap_or(60);
                        self.display_cache
                            .insert(connector.clone(), DisplayData(backend, value));
                        return;
                    }
                }
            }
        }
        let res = Self::ddcutil_bus_for_connector(&connector);
        if let Ok(path) = res
            && let Ok(data) = fs::metadata(path)
        {
            let id = data.rdev();
            println!("Found DDC/CI display {connector}");
            for display in ddc_hi::Display::enumerate() {
                if id.to_string().eq(&display.info.id) {
                    let mut backend = DisplayBackend::DdcCi(display);
                    let value = backend.get_brightness().unwrap_or(60);
                    self.display_cache
                        .insert(connector.clone(), DisplayData(backend, value));
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

            if drm_connector.as_deref() == Some(connector) {
                if let Some(bus) = bus {
                    return Ok(bus);
                }
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No DDC-capable display found for {connector}"),
        )
        .into())
    }
}
