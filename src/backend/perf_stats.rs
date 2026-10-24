use crate::LOGGER;
use crate::backend::{BackendThread, Registrator};
use all_smi::AllSmi;
use battery::State;
use battery::units::Time;
use battery::units::power::watt;
use battery::units::ratio::percent;
use battery::units::time::millisecond;
use crossbeam_channel::{Receiver, Sender};
use jmb_logger_rs::*;
use std::thread;
use std::time::Duration;
use sysinfo::{Disks, System};

const POLL_INTERVAL: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct Stats {
    pub cpu: u8,
    pub ram: u8,
    pub disks: Vec<(String, u8)>,
    pub gpus: Vec<(String, u8)>,
    pub battery_charge: u8,
    pub battery_state: State,
    pub battery_draw: f32,
    pub battery_time: u64,
}

pub struct StatsBackend {
    subscribers: Vec<Sender<Stats>>,
    system: System,
    disks: Disks,
}

impl BackendThread<Stats, ()> for StatsBackend {
    fn subscribe(registrator: Registrator<Stats>) -> (Receiver<Stats>, ()) {
        let (tx, rx) = crossbeam_channel::unbounded::<Stats>();
        let _ = registrator.send(tx);
        (rx, ())
    }

    fn subscribers(&self) -> &Vec<Sender<Stats>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<Stats>>) {
        thread::spawn(move || {
            let mut instance = Self {
                subscribers: Vec::new(),
                system: System::new(),
                disks: Disks::new(),
            };
            let manager = battery::Manager::new();
            let smi = AllSmi::new();
            loop {
                while let Ok(tx) = registration_rx.try_recv() {
                    trace!(&LOGGER, "Added Stats Subscriber: {tx:?}");
                    instance.subscribers.push(tx);
                }

                instance.system.refresh_cpu_usage();
                instance.system.refresh_memory();
                instance.disks.refresh(true);

                let cpu = instance.system.global_cpu_usage() as u8;
                let ram =
                    (instance.system.used_memory() * 100 / instance.system.total_memory()) as u8;

                let disk_stats: Vec<(String, u8)> = instance
                    .disks
                    .iter()
                    .map(|disk| {
                        let mount_point = disk.mount_point().display().to_string();
                        let used_space = disk.total_space() - disk.available_space();
                        let percentage = (used_space * 100 / disk.total_space()) as u8;
                        (mount_point, percentage)
                    })
                    .collect();

                let mut gpus: Vec<(String, u8)> = Vec::new();
                if let Ok(smi) = &smi {
                    gpus = smi
                        .get_gpu_info()
                        .iter()
                        .map(|info| (info.name.clone(), info.utilization as u8))
                        .collect();
                }

                let mut battery_charge: u8 = 255; // Use > 100 as invalid value
                let mut battery_state = State::Unknown;
                let mut battery_draw: f32 = 0.0;
                let mut battery_time: u64 = 0;

                if let Ok(manager) = &manager
                    && let Ok(mut batteries) = manager.batteries()
                {
                    match batteries.next() {
                        Some(Ok(battery)) => {
                            battery_charge =
                                battery.state_of_charge().get::<percent>().round() as u8;
                            battery_state = battery.state();
                            battery_draw = battery.energy_rate().get::<watt>();
                            battery_time = if battery_state == State::Charging {
                                battery
                                    .time_to_full()
                                    .unwrap_or(Time::new::<millisecond>(0.0))
                                    .get::<millisecond>() as u64
                            } else if battery_state == State::Discharging {
                                battery
                                    .time_to_empty()
                                    .unwrap_or(Time::new::<millisecond>(0.0))
                                    .get::<millisecond>() as u64
                            } else {
                                0
                            };
                        }
                        Some(Err(e)) => error!(&LOGGER, "Error accessing battery: {e:?}"),
                        None => {}
                    }
                }

                let msg = Stats {
                    cpu,
                    ram,
                    disks: disk_stats,
                    gpus,
                    battery_charge,
                    battery_state,
                    battery_draw,
                    battery_time,
                };
                instance.publish(msg);

                thread::sleep(Duration::from_millis(POLL_INTERVAL));
            }
        });
    }
}
