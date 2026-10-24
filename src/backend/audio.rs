use crate::LOGGER;
use crate::backend::audio::ControllerMsg::Request;
use crate::backend::{BackendThread, Registrator};
use crossbeam_channel::{Receiver, Sender};
use eventfd::{EfdFlags, EventFD};
use jmb_logger_rs::*;
use mainloop::standard::Mainloop;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::{Facility, InterestMaskSet};
use pulse::context::{Context, FlagSet, State};
use pulse::mainloop::api;
use pulse::mainloop::events::io;
use pulse::proplist::Proplist;
use pulse::volume::{ChannelVolumes, Volume};
use pulse::{mainloop, operation};
use std::boxed::Box;
use std::cell::Cell;
use std::os::fd::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct AudioController {
    pub mainloop: Mainloop,
    context: Context,
    device_index: Option<u32>,
    device_bt: Option<bool>,
    current_vol: u8,
    current_mute: bool,
}

#[derive(Debug)]
pub enum ControllerMsg {
    NotifyServer,
    NotifySink(u32),
    Request,
    Set(u8, bool),
}

impl AudioController {
    pub fn new(tx: Sender<ControllerMsg>) -> Self {
        let mut mainloop = Mainloop::new().unwrap();
        let mut prop_list = Proplist::new().unwrap();

        prop_list
            .set_str(pulse::proplist::properties::APPLICATION_NAME, "jmb-bar")
            .unwrap();

        let mut context = Context::new_with_proplist(&mainloop, "jmb-bar", &prop_list).unwrap();

        context.connect(None, FlagSet::NOFLAGS, None).unwrap();

        loop {
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => panic!("PulseAudio connection failed"),
                _ => mainloop.iterate(false),
            };
        }

        let device_list = Arc::new(Mutex::new(Vec::new()));
        let clone = Arc::clone(&device_list);
        let op = context
            .introspect()
            .get_sink_info_list(move |info| match info {
                ListResult::Item(device) => {
                    if let Some(description) = device.description.as_ref()
                        && let Some(name) = device.name.as_ref()
                    {
                        if let Ok(mut lock) = clone.lock() {
                            lock.push((name.to_string(), description.to_string()));
                        } else {
                            error!(&LOGGER, "Failed to unlock device list on device {}", name);
                        }
                    } else {
                        error!(&LOGGER, "Failed to access device description");
                    }
                }
                ListResult::End => {}
                ListResult::Error => {}
            });

        while op.get_state() == operation::State::Running {
            mainloop.iterate(false);
        }

        context.set_subscribe_callback(Some(Box::new(move |facility, _, index| {
            if facility.is_some_and(|f| f == Facility::Sink) {
                let _ = tx.send(ControllerMsg::NotifySink(index));
            } else if facility.is_some_and(|f| f == Facility::Server) {
                let _ = tx.send(ControllerMsg::NotifyServer);
            }
        })));
        let _op = context.subscribe(InterestMaskSet::ALL, |success| {
            trace!(&LOGGER, "Subscribed to pulseaudio events: {success}");
        });

        let mut controller = AudioController {
            context,
            mainloop,
            device_index: None,
            device_bt: None,
            current_vol: 0,
            current_mute: true,
        };

        controller.query_default_device_blocking();
        controller
    }

    pub fn query_default_device_blocking(&mut self) {
        let default_device = Arc::new(Mutex::new(None));
        let dev_clone = default_device.clone();
        let op = self.context.introspect().get_server_info(move |info| {
            if let Some(name) = info.default_sink_name.as_ref() {
                let _ = dev_clone
                    .lock()
                    .expect("Couldn't get lock on default dev")
                    .insert(name.to_string());
            }
        });

        while op.get_state() == operation::State::Running {
            self.mainloop.iterate(false);
        }

        let name = default_device
            .lock()
            .expect("Couldn't get lock on device name")
            .take()
            .unwrap_or(String::new());

        let index = Arc::new(Mutex::new(None));
        let index_clone = index.clone();
        let op = self
            .context
            .introspect()
            .get_sink_info_by_name(&name, move |res| match res {
                ListResult::Item(item) => {
                    let _ = index_clone
                        .lock()
                        .expect("Couldn't get lock on device index")
                        .insert(item.index);
                }
                ListResult::End => {}
                ListResult::Error => {}
            });

        while op.get_state() == operation::State::Running {
            self.mainloop.iterate(false);
        }

        let val = index
            .lock()
            .expect("Couldn't get lock on device index")
            .take();

        self.device_index = val;
        self.device_bt = Some(name.starts_with("bluez"));
        debug!(
            &LOGGER,
            "Found default Audio Sink Idx: {:?} Bluetooth: {}",
            self.device_index,
            self.device_bt.unwrap()
        );
    }

    pub fn query_state_blocking(&mut self) -> bool {
        if let Some(idx) = self.device_index {
            let volume = Rc::new(Cell::new(self.current_vol));
            let vol_clone = volume.clone();
            let mute = Rc::new(Cell::new(self.current_mute));
            let mute_clone = mute.clone();
            let op =
                self.context
                    .introspect()
                    .get_sink_info_by_index(idx, move |info| match info {
                        ListResult::Item(info) => {
                            let percent =
                                info.volume.avg().0 as f64 * 100.0 / Volume::NORMAL.0 as f64;
                            trace!(
                                &LOGGER,
                                "State of default Sink changed: {percent} {}", info.mute
                            );
                            vol_clone.set(percent as u8);
                            mute_clone.set(info.mute);
                        }
                        ListResult::End => {}
                        ListResult::Error => {}
                    });

            while op.get_state() == operation::State::Running {
                self.mainloop.iterate(false);
            }

            let mut dirty = false;
            let volume = volume.take();
            if self.current_vol != volume {
                self.current_vol = volume;
                dirty = true;
            }
            let mute = mute.take();
            if self.current_mute != mute {
                self.current_mute = mute;
                dirty = true;
            }
            return dirty;
        }
        false
    }

    pub fn set_state(&mut self, volume: u8, mute: bool) {
        if let Some(idx) = self.device_index {
            if self.current_vol != volume {
                self.current_vol = volume;
                let volume = Volume(
                    ((volume.min(150) as f64 / 100.0) * Volume::NORMAL.0 as f64).round() as u32,
                );

                let mut channel_volumes = ChannelVolumes::default();
                channel_volumes.set(2, volume);

                self.context.introspect().set_sink_volume_by_index(
                    idx,
                    &channel_volumes,
                    Some(Box::new(|success| {
                        if !success {
                            warn!(&LOGGER, "Failed setting sink volume");
                        }
                    })),
                );
            }

            if self.current_mute != mute {
                self.current_mute = mute;
                let a = self.context.introspect().set_sink_mute_by_index(
                    idx,
                    mute,
                    Some(Box::new(|success| {
                        if !success {
                            warn!(&LOGGER, "Failed setting sink mute");
                        }
                    })),
                );
                while a.get_state() == operation::State::Running {
                    self.mainloop.iterate(false);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum MediaCommand {
    Init(Sender<MediaCommand>),
    Request,
    Cmd(u8, bool, bool),
}

pub struct MediaBackend {
    subscribers: Vec<Sender<MediaCommand>>,
    command_tx: Sender<MediaCommand>,
}

impl BackendThread<MediaCommand, Sender<MediaCommand>> for MediaBackend {
    fn subscribe(
        registrator: Registrator<MediaCommand>,
    ) -> (Receiver<MediaCommand>, Sender<MediaCommand>) {
        let (result_tx, result_rx) = crossbeam_channel::unbounded::<MediaCommand>();
        let _ = registrator.send(result_tx);
        while let Ok(state) = result_rx.recv() {
            return match state {
                MediaCommand::Init(sender) => (result_rx, sender),
                _ => continue,
            };
        }
        panic!("Channel closed unexpectedly, without returning anything")
    }

    fn subscribers(&self) -> &Vec<Sender<MediaCommand>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<MediaCommand>>) {
        let (command_tx, command_rx) = crossbeam_channel::unbounded::<MediaCommand>();
        let instance = Arc::new(Mutex::new(Self {
            subscribers: vec![],
            command_tx,
        }));
        let instance_clone = instance.clone();
        let (controller_tx, controller_rx) = crossbeam_channel::unbounded::<ControllerMsg>();
        let controller_tx_clone = controller_tx.clone();
        let notify = Arc::new(EventFD::new(0, EfdFlags::EFD_CLOEXEC).unwrap());
        let notify_clone = notify.clone();
        thread::spawn(move || {
            let mut controller = AudioController::new(controller_tx_clone);
            let _event = api::Mainloop::new_io_event(
                &mut controller.mainloop,
                notify_clone.as_raw_fd(),
                io::FlagSet::INPUT,
                Box::new(move |_, fd, _flags| match drain_eventfd(fd) {
                    Ok(_) => trace!(&LOGGER, "Woke pulseaudio loop using EventFD"),
                    Err(e) => warn!(&LOGGER, "Error draining EventFD {}", e.to_string()),
                }),
            )
            .unwrap();
            loop {
                while let Ok(msg) = controller_rx.try_recv() {
                    match msg {
                        Request => {
                            instance_clone.lock().expect("Couldn't get lock").publish(
                                MediaCommand::Cmd(
                                    controller.current_vol,
                                    controller.current_mute,
                                    controller.device_bt.unwrap_or(false),
                                ),
                            );
                        }
                        ControllerMsg::NotifyServer => {
                            controller.query_default_device_blocking();
                            let _ = controller.query_state_blocking();
                            instance_clone.lock().expect("Couldn't get lock").publish(
                                MediaCommand::Cmd(
                                    controller.current_vol,
                                    controller.current_mute,
                                    controller.device_bt.unwrap_or(false),
                                ),
                            );
                        }
                        ControllerMsg::NotifySink(idx) => {
                            if controller.device_index.is_some_and(|i| i == idx) {
                                let changed = controller.query_state_blocking();
                                if changed {
                                    instance_clone.lock().expect("Couldn't get lock").publish(
                                        MediaCommand::Cmd(
                                            controller.current_vol,
                                            controller.current_mute,
                                            controller.device_bt.unwrap_or(false),
                                        ),
                                    );
                                    trace!(
                                        &LOGGER,
                                        "Change: {} muted: {}",
                                        controller.current_vol,
                                        controller.current_mute
                                    );
                                }
                            }
                        }
                        ControllerMsg::Set(volume, mute) => {
                            controller.set_state(volume, mute);
                            instance_clone.lock().expect("Couldn't get lock").publish(
                                MediaCommand::Cmd(
                                    controller.current_vol,
                                    controller.current_mute,
                                    controller.device_bt.unwrap_or(false),
                                ),
                            );
                        }
                    }
                }
                controller.mainloop.iterate(true);
            }
        });
        let instance_clone = instance.clone();
        thread::spawn(move || {
            while let Ok(sender) = registration_rx.recv() {
                if sender
                    .send(MediaCommand::Init(
                        instance_clone
                            .lock()
                            .expect("Couldn't get lock")
                            .command_tx
                            .clone(),
                    ))
                    .is_ok()
                {
                    let _ = sender.send(MediaCommand::Request);
                    instance_clone
                        .lock()
                        .expect("Couldn't get subscribers")
                        .subscribers
                        .push(sender);
                }
            }
        });
        thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    MediaCommand::Cmd(vol, mute, _) => {
                        let _ = controller_tx.send(ControllerMsg::Set(vol, mute));
                        notify.write(1).expect("Error writing notify");
                    }
                    MediaCommand::Request => {
                        let _ = controller_tx.send(Request);
                        notify.write(1).expect("Error writing notify");
                    }
                    _ => {
                        warn!(&LOGGER, "Frontend sent invalid Command")
                    }
                }
            }
        });
    }
}

fn drain_eventfd(fd: RawFd) -> std::io::Result<u64> {
    let mut value: u64 = 0;

    let result = unsafe { libc::read(fd, (&mut value as *mut u64).cast(), size_of::<u64>()) };

    if result == size_of::<u64>() as isize {
        Ok(value)
    } else if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "short eventfd read",
        ))
    }
}
