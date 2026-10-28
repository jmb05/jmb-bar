pub mod audio;
pub mod brightness_control;
pub(crate) mod idle;
pub(crate) mod json_socket;
pub mod network;
pub mod perf_stats;
pub mod window_manager_hypr;

use crate::LOGGER;
use crate::backend::audio::{MediaBackend, MediaCommand};
use crate::backend::brightness_control::{BrightnessManager, BrightnessMessage};
use crate::backend::idle::{IdleDaemon, IdleMessage, IdleState};
use crate::backend::json_socket::{JsonSocketBackend, State};
use crate::backend::network::{NetworkBackendHandle, NetworkCommand, NetworkState};
use crate::backend::perf_stats::{Stats, StatsBackend};
use crate::backend::window_manager_hypr::{HyprlandComms, WindowManagerEvent};
use crossbeam_channel::{Receiver, Sender};
use jmb_logger_rs::*;
use std::fmt::Debug;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) type Registrator<M> = Sender<Sender<M>>;

pub trait BackendThread<M: Debug + Clone, D> {
    fn subscribe(registrator: Registrator<M>) -> (Receiver<M>, D);
    fn subscribers(&self) -> &Vec<Sender<M>>;
    fn init(registration_rx: Receiver<Sender<M>>);
    fn publish(&self, message: M) {
        trace!(&LOGGER, "Sending: {message:?} to {:?}", self.subscribers());
        for tx in self.subscribers() {
            if let Err(e) = tx.send(message.clone()) {
                warn!(
                    &LOGGER,
                    "Error sending message to frontend: {}",
                    e.to_string()
                );
            }
        }
    }
    fn start() -> Registrator<M> {
        let (registration_tx, registration_rx) = crossbeam_channel::unbounded::<Sender<M>>();
        Self::init(registration_rx);
        registration_tx
    }
}

pub trait SimpleBackendThread<M: Debug + Clone>: BackendThread<M, ()> {
    fn subscribers(&self) -> &Vec<Sender<M>>;
    fn init(registration_rx: Receiver<Sender<M>>);
}

impl<M: Debug + Clone, S: SimpleBackendThread<M>> BackendThread<M, ()> for S {
    fn subscribe(registrator: Registrator<M>) -> (Receiver<M>, ()) {
        let (tx, rx) = crossbeam_channel::unbounded::<M>();
        let _ = registrator.send(tx);
        (rx, ())
    }

    fn subscribers(&self) -> &Vec<Sender<M>> {
        SimpleBackendThread::subscribers(self)
    }

    fn init(registration_rx: Receiver<Sender<M>>) {
        <S as SimpleBackendThread<M>>::init(registration_rx)
    }
}

#[derive(Clone)]
pub struct Backend {
    hypr_registrator: Registrator<WindowManagerEvent>,
    net_registrator: Registrator<NetworkState>,
    br_registrator: Registrator<BrightnessMessage>,
    stats_registrator: Registrator<Stats>,
    media_registrator: Registrator<MediaCommand>,
    tailscale_registrator: Registrator<State>,
    idle_registrator: Registrator<IdleState>,
}

impl Backend {
    pub fn start() -> Self {
        debug!(&LOGGER, "Starting Backend threads...");
        let hypr_registrator = HyprlandComms::start();
        let net_registrator = NetworkBackendHandle::start();
        let br_registrator = BrightnessManager::start();
        let stats_registrator = StatsBackend::start();
        let media_registrator = MediaBackend::start();
        let tailscale_registrator = JsonSocketBackend::start();
        let idle_registrator = IdleDaemon::start();
        debug!(&LOGGER, "Started Backend threads");
        Self {
            hypr_registrator,
            net_registrator,
            br_registrator,
            stats_registrator,
            media_registrator,
            tailscale_registrator,
            idle_registrator,
        }
    }

    pub fn subscribe(&self) -> BackendChannels {
        let (net_rx, net_tx) = NetworkBackendHandle::subscribe(self.net_registrator.clone());
        let (hypr_rx, _) = HyprlandComms::subscribe(self.hypr_registrator.clone());
        let (br_rx, br_tx) = BrightnessManager::subscribe(self.br_registrator.clone());
        let (stats_rx, _) = StatsBackend::subscribe(self.stats_registrator.clone());
        let (media_rx, media_tx) = MediaBackend::subscribe(self.media_registrator.clone());
        let (tailscale_rx, _) = JsonSocketBackend::subscribe(self.tailscale_registrator.clone());
        let (idle_rx, idle_tx) = IdleDaemon::subscribe(self.idle_registrator.clone());
        BackendChannels {
            net_rx: Some(net_rx),
            net_tx,
            hypr_rx: Some(hypr_rx),
            br_rx: Some(br_rx),
            br_tx,
            stats_rx: Some(stats_rx),
            media_rx: Some(media_rx),
            media_tx,
            tailscale_rx: Some(tailscale_rx),
            idle_rx: Some(idle_rx),
            idle_tx,
        }
    }
}

pub struct BackendChannels {
    net_rx: Option<Receiver<NetworkState>>,
    net_tx: UnboundedSender<NetworkCommand>,
    hypr_rx: Option<Receiver<WindowManagerEvent>>,
    br_rx: Option<Receiver<BrightnessMessage>>,
    br_tx: Sender<BrightnessMessage>,
    stats_rx: Option<Receiver<Stats>>,
    media_rx: Option<Receiver<MediaCommand>>,
    media_tx: Sender<MediaCommand>,
    tailscale_rx: Option<Receiver<State>>,
    idle_rx: Option<Receiver<IdleState>>,
    idle_tx: Sender<IdleMessage>,
}

impl BackendChannels {
    pub fn take_net_rx(&mut self) -> Option<Receiver<NetworkState>> {
        self.net_rx.take()
    }
    pub fn take_hypr_rx(&mut self) -> Option<Receiver<WindowManagerEvent>> {
        self.hypr_rx.take()
    }
    pub fn take_br_rx(&mut self) -> Option<Receiver<BrightnessMessage>> {
        self.br_rx.take()
    }
    pub fn take_stats_rx(&mut self) -> Option<Receiver<Stats>> {
        self.stats_rx.take()
    }
    pub fn take_netbird_rx(&mut self) -> Option<Receiver<State>> {
        self.tailscale_rx.take()
    }
    pub fn take_media_rx(&mut self) -> Option<Receiver<MediaCommand>> {
        self.media_rx.take()
    }
    pub fn take_idle_rx(&mut self) -> Option<Receiver<IdleState>> {
        self.idle_rx.take()
    }
    pub fn net_tx(&self) -> UnboundedSender<NetworkCommand> {
        self.net_tx.clone()
    }
    pub fn br_tx(&self) -> Sender<BrightnessMessage> {
        self.br_tx.clone()
    }
    pub fn media_tx(&self) -> Sender<MediaCommand> {
        self.media_tx.clone()
    }
    pub fn idle_tx(&self) -> Sender<IdleMessage> {
        self.idle_tx.clone()
    }
}
