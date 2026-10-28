use crate::backend::{BackendThread, Registrator};
use crate::{APP_NAME, LOGGER};
use crossbeam_channel::{Receiver, Sender};
use jmb_logger_rs::*;
use nmrs::raw::zbus::interface;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    Event, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use zbus::conn::Builder;
use zbus::message::Header;

const SERVICE: &str = "org.freedesktop.ScreenSaver";
const OBJECT_PATH: &str = "/org/freedesktop/ScreenSaver";
const LEGACY_OBJECT_PATH: &str = "/ScreenSaver";

struct WlState {
    idle: Option<bool>,
    tx: watch::Sender<IdleStateInner>,
}

delegate_noop!(WlState: ignore WlSeat);

impl Dispatch<WlRegistry, wayland_client::globals::GlobalListContents> for WlState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            Event::Idled => state.idle = Some(true),
            Event::Resumed => state.idle = Some(false),
            _ => {}
        }
        if let Some(idle) = state.idle {
            state.tx.send_modify(|state| {
                state.wayland_idle = idle;
            });
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, (), WlState> for WlState {
    fn event(
        _: &mut WlState,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WlState>,
    ) {
    }
}

#[derive(Debug)]
struct Inhibitor {
    owner: String,
    application: String,
    _reason: String,
}

#[derive(Clone, Copy, Debug)]
struct IdleStateInner {
    dimmed: bool,
    wayland_idle: bool,
    dbus_inhibited: bool,
    caffeine: bool,
}

impl IdleStateInner {
    fn should_dim(&self) -> bool {
        self.wayland_idle && !self.dbus_inhibited
    }
}

#[derive(Debug, Clone)]
pub enum IdleState {
    Init(Sender<IdleMessage>),
    Dimmed,
    Inhibited,
    Caffeine,
    Active,
}

impl From<IdleStateInner> for IdleState {
    fn from(value: IdleStateInner) -> Self {
        if value.should_dim() {
            IdleState::Dimmed
        } else if value.caffeine {
            IdleState::Caffeine
        } else if value.dbus_inhibited {
            IdleState::Inhibited
        } else {
            IdleState::Active
        }
    }
}

#[derive(Debug, Clone)]
pub enum IdleMessage {
    Inhibit,
    Release,
}

#[derive(Debug)]
pub struct IdleDaemon {
    subscribers: Vec<Sender<IdleState>>,
}

impl BackendThread<IdleState, Sender<IdleMessage>> for IdleDaemon {
    fn subscribe(
        registrator: Registrator<IdleState>,
    ) -> (Receiver<IdleState>, Sender<IdleMessage>) {
        let (tx, rx) = crossbeam_channel::unbounded::<IdleState>();
        let _ = registrator.send(tx);
        while let Ok(message) = rx.recv() {
            return match message {
                IdleState::Init(sender) => (rx, sender),
                _ => continue,
            };
        }
        panic!("Channel closed unexpectedly, without returning anything")
    }

    fn subscribers(&self) -> &Vec<Sender<IdleState>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<IdleState>>) {
        let (tx, rx) = crossbeam_channel::unbounded::<IdleMessage>();
        let tx_clone = tx.clone();
        let instance = Arc::new(Mutex::new(Self {
            subscribers: Vec::new(),
        }));
        let instance_clone = instance.clone();
        std::thread::spawn(move || {
            while let Ok(sender) = registration_rx.recv() {
                if sender.send(IdleState::Init(tx_clone.clone())).is_ok() {
                    trace!(&LOGGER, "Added IdleDaemon Subscriber");
                    instance_clone
                        .lock()
                        .expect("Couldn't get lock")
                        .subscribers
                        .push(sender);
                }
            }
        });
        let (state_tx, state_rx) = watch::channel(IdleStateInner {
            dimmed: false,
            wayland_idle: false,
            dbus_inhibited: false,
            caffeine: false,
        });
        let tx_clone = state_tx.clone();
        std::thread::spawn(move || {
            if let Err(err) = Self::start_idle_notifier(tx_clone) {
                error!(&LOGGER, "Idle Daemon unexpectedly quit, reason: {err}");
            }
        });
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");
            runtime.block_on(async move {
                let mut idle_inhibitor = None;
                while let Ok(msg) = rx.recv() {
                    match msg {
                        IdleMessage::Inhibit => {
                            if idle_inhibitor.is_none()
                                && let Ok(ii) = IdleInhibitor::inhibit_idle().await
                            {
                                idle_inhibitor = Some(ii);
                            }
                        }
                        IdleMessage::Release => {
                            if let Some(ii) = idle_inhibitor.take()
                                && let Err(e) = ii.uninhibit_idle().await
                            {
                                warn!(&LOGGER, "Error uninhibiting {e}");
                            }
                        }
                    }
                }
            });
        });
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            runtime.block_on(async move {
                let res = IdleDaemon::start_dbus(state_tx).await;
                match res {
                    Ok((_, _)) => {
                        Self::run_idle_timer(instance, state_rx, Duration::from_secs(10)).await;
                    }
                    Err(err) => error!(&LOGGER, "Error running D-Bus Screensaver: {err}"),
                }
            });
        });
    }
}

impl IdleDaemon {
    async fn run_idle_timer(
        instance: Arc<Mutex<Self>>,
        mut state_rx: watch::Receiver<IdleStateInner>,
        timeout: Duration,
    ) {
        loop {
            let mut state = *state_rx.borrow_and_update();
            instance.lock().unwrap().publish(state.into());

            if !state.should_dim() {
                if state_rx.changed().await.is_err() {
                    return;
                }

                continue;
            }

            tokio::select! {
                _ = sleep(timeout) => {
                    state.dimmed = true;
                    instance.lock().unwrap().publish(state.into());
                }
                result = state_rx.changed() => {
                    if result.is_err() {
                        return;
                    }
                    state.dimmed = false;
                    instance.lock().unwrap().publish(state.into());
                }
            }
        }
    }

    fn start_idle_notifier(tx: watch::Sender<IdleStateInner>) -> Result<(), Box<dyn Error>> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut event_queue) = registry_queue_init::<WlState>(&conn)?;
        let queue_handle = event_queue.handle();

        let seat = globals.bind::<WlSeat, _, _>(&queue_handle, 1..=9, ())?;

        let notifier = globals.bind::<ExtIdleNotifierV1, _, _>(&queue_handle, 2..=2, ())?;

        let mut state = WlState { idle: None, tx };

        let _notification = notifier.get_idle_notification(10_000, &seat, &queue_handle, ());

        loop {
            event_queue.blocking_dispatch(&mut state)?;
        }
    }

    async fn start_dbus(
        tx: watch::Sender<IdleStateInner>,
    ) -> zbus::Result<(ScreenSaver, zbus::connection::Connection)> {
        let service = ScreenSaver::new(tx);

        let connection = Builder::session()?
            .name(SERVICE)?
            .serve_at(OBJECT_PATH, service.clone())?
            .serve_at(LEGACY_OBJECT_PATH, service.clone())?
            .build()
            .await?;

        info!(&LOGGER, "Providing {SERVICE}");

        Ok((service, connection))
    }
}

pub struct IdleInhibitor {
    connection: zbus::Connection,
    cookie: u32,
}

impl IdleInhibitor {
    async fn inhibit_idle() -> zbus::Result<IdleInhibitor> {
        let connection = zbus::Connection::session().await?;

        let reply = connection
            .call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "Inhibit",
                &(APP_NAME, "Caffeine mode active"),
            )
            .await?;

        let cookie = reply.body().deserialize::<u32>()?;

        Ok(Self { connection, cookie })
    }

    async fn uninhibit_idle(self) -> zbus::Result<()> {
        self.connection
            .call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "UnInhibit",
                &(self.cookie,),
            )
            .await?;

        Ok(())
    }
}

#[derive(Clone)]
struct ScreenSaver {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    next_cookie: u32,
    inhibitors: HashMap<u32, Inhibitor>,
    tx: watch::Sender<IdleStateInner>,
}

impl ScreenSaver {
    fn new(tx: watch::Sender<IdleStateInner>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_cookie: 1,
                inhibitors: HashMap::new(),
                tx,
            })),
        }
    }
}

#[interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    #[zbus(name = "Inhibit")]
    fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        application_name: String,
        reason_for_inhibit: String,
    ) -> zbus::fdo::Result<u32> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing D-Bus sender".into()))?
            .to_string();

        let mut inner = self.inner.lock().unwrap();

        let start = inner.next_cookie;
        let cookie = loop {
            let candidate = inner.next_cookie;
            inner.next_cookie = inner.next_cookie.wrapping_add(1).max(1);

            if !inner.inhibitors.contains_key(&candidate) {
                break candidate;
            }

            if inner.next_cookie == start {
                return Err(zbus::fdo::Error::Failed(
                    "No available inhibition cookies".into(),
                ));
            }
        };

        let caffeine_flag = application_name.eq(APP_NAME);

        debug!(&LOGGER, "Inhibitor added: {application_name} {cookie}");
        inner.inhibitors.insert(
            cookie,
            Inhibitor {
                owner,
                application: application_name,
                _reason: reason_for_inhibit,
            },
        );

        inner.tx.send_modify(|state| {
            state.dbus_inhibited = !inner.inhibitors.is_empty();
            state.caffeine = caffeine_flag;
        });
        Ok(cookie)
    }

    #[zbus(name = "UnInhibit")]
    fn uninhibit(&self, #[zbus(header)] header: Header<'_>, cookie: u32) -> zbus::fdo::Result<()> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing D-Bus sender".into()))?
            .to_string();

        let mut inner = self.inner.lock().unwrap();

        match inner.inhibitors.get(&cookie) {
            Some(inhibitor) if inhibitor.owner == owner => {
                if let Some(removed) = inner.inhibitors.remove(&cookie) {
                    debug!(
                        &LOGGER,
                        "Inhibitor removed: {} {cookie}", removed.application
                    );
                }
                let mut caffeine_flag = false;
                for inhibitor in inner.inhibitors.values() {
                    if inhibitor.application.eq(APP_NAME) {
                        caffeine_flag = true;
                        break;
                    }
                }
                inner.tx.send_modify(|state| {
                    state.dbus_inhibited = !inner.inhibitors.is_empty();
                    state.caffeine = caffeine_flag;
                });
                Ok(())
            }
            Some(_) => Err(zbus::fdo::Error::AccessDenied(
                "Cookie belongs to another D-Bus client".into(),
            )),
            None => Err(zbus::fdo::Error::Failed("Unknown inhibition cookie".into())),
        }
    }
}
