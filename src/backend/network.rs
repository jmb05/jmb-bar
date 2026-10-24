use crate::LOGGER;
use crate::backend::BackendThread;
use crossbeam_channel::{Receiver, Sender};
use futures_util::StreamExt;
use jmb_logger_rs::*;
use nmrs::raw::zbus;
use nmrs::raw::zbus::Error;
use nmrs::raw::zbus::names::{ErrorName, InterfaceName};
use nmrs::raw::zvariant::OwnedObjectPath;
use nmrs::{ActiveConnection, ConnectionError, NetworkEvent, NetworkManager, WifiSecurity};
use std::cmp::Ordering;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Instant, Sleep};

#[derive(Debug, Clone)]
pub enum NetworkType {
    Wifi,
    Wired,
}

#[derive(Debug, Clone)]
pub struct NetworkInfo {
    pub name: String,
    pub network_type: NetworkType,
    pub signal: u8,
    pub secured: Option<bool>,
    pub known: bool,
}

impl Eq for NetworkInfo {}

impl PartialEq<Self> for NetworkInfo {
    fn eq(&self, other: &Self) -> bool {
        other.name.eq(&self.name)
    }
}

impl PartialOrd<Self> for NetworkInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(other.cmp(self))
    }
}

impl Ord for NetworkInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.signal
            .cmp(&other.signal)
            .then_with(|| self.name.cmp(&other.name))
    }
}

pub enum NetworkCommand {
    ToggleVpn(String),
    Connect(String, Auth),
}

#[derive(Clone, Debug)]
pub enum Auth {
    Open,
    Password(String),
}

#[derive(Clone, Debug)]
pub enum AuthType {
    Password,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Authentication Failure")]
    AuthFailed,
    #[error("Network not found")]
    NotFound,
    #[error("Unknown error")]
    Unknown,
}

impl From<ConnectionError> for ConnectError {
    fn from(value: ConnectionError) -> Self {
        match value {
            ConnectionError::AuthFailed => ConnectError::AuthFailed,
            ConnectionError::NotFound => ConnectError::NotFound,
            ConnectionError::Dbus(err) => match err {
                Error::MethodError(name, ..) => {
                    if name.eq(&ErrorName::try_from(
                        "org.freedesktop.NetworkManager.Device.SpecificObjectNotFound",
                    )
                    .unwrap())
                    {
                        ConnectError::NotFound
                    } else {
                        ConnectError::Unknown
                    }
                }
                _ => ConnectError::Unknown,
            },
            _ => ConnectError::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub enum NetworkState {
    Init(UnboundedSender<NetworkCommand>),
    ChangeNetwork(Option<NetworkInfo>),
    VpnState { profile: String, connected: bool },
    Networks(Vec<NetworkInfo>),
    Auth(String, AuthType),
    ConnectFail(String, ConnectError),
    ConnectSuccess(String),
}

#[derive(Debug, Clone)]
pub struct NetworkBackendHandle {
    subscribers: Vec<Sender<NetworkState>>,
    command_tx: UnboundedSender<NetworkCommand>,
}

struct EventDebouncer {
    duration: Duration,
    deadline: Option<Instant>,
    sleep: Pin<Box<Sleep>>,
}

impl BackendThread<NetworkState, UnboundedSender<NetworkCommand>> for NetworkBackendHandle {
    fn subscribe(
        registrator: Sender<Sender<NetworkState>>,
    ) -> (Receiver<NetworkState>, UnboundedSender<NetworkCommand>) {
        let (result_tx, result_rx) = crossbeam_channel::unbounded::<NetworkState>();
        let _ = registrator.send(result_tx);
        while let Ok(state) = result_rx.recv() {
            return match state {
                NetworkState::Init(sender) => (result_rx, sender),
                _ => continue,
            };
        }
        panic!("Channel closed unexpectedly, without returning anything")
    }

    fn subscribers(&self) -> &Vec<Sender<NetworkState>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<NetworkState>>) {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<NetworkCommand>();
        let mut instance = Self {
            subscribers: vec![],
            command_tx,
        };
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            runtime.block_on(async move {
                let nm = match NetworkManager::new().await {
                    Ok(nm) => nm,
                    Err(err) => {
                        error!(&LOGGER, "Error getting NetworkManager: {}", err.to_string());
                        return;
                    }
                };

                let mut events = match nm.network_events().await {
                    Ok(events) => events,
                    Err(err) => {
                        error!(&LOGGER, "Error getting NetworkManager events: {}", err.to_string());
                        return;
                    }
                };


                let mut debouncer = EventDebouncer {
                    duration: Duration::from_millis(200),
                    deadline: None,
                    sleep: Box::pin(tokio::time::sleep(Duration::from_secs(86400))),
                };
                debouncer.sleep.as_mut().reset(Instant::now() + Duration::from_secs(86400));
                loop {
                    while let Ok(tx) = registration_rx.try_recv() {
                        if tx.send(NetworkState::Init(instance.command_tx.clone())).is_ok() {
                            instance.subscribers.push(tx);
                            let _ = instance.send_all_networks(&nm).await;
                            let _ = instance.send_active_connection(&nm).await;
                        }
                    }
                    tokio::select! {
                        command = command_rx.recv() => {
                            let Some(command) = command else {
                                break;
                            };

                            // Run the command without stopping event processing
                            // while a potentially slow connection operation runs.
                            let nm = nm.clone();
                            let instance_clone = instance.clone();

                            tokio::spawn(async move {
                                match command {
                                    NetworkCommand::ToggleVpn(profile) => instance_clone.toggle_vpn(profile, &nm).await,
                                    NetworkCommand::Connect(ssid, auth) => {
                                        if let Ok(saved) = nm.list_saved_connections().await {
                                            if saved.iter().find(|c| c.id.eq(&ssid)).is_some() {
                                                instance_clone.connect(&nm, ssid, WifiSecurity::Open).await;
                                            } else {
                                                match auth {
                                                    Auth::Open => {
                                                        if let Ok(networks) = nm.list_access_points(None).await {
                                                            if let Some(ap) = networks.iter().find(|net| net.ssid.eq(&ssid)) {
                                                                if ap.security.is_open() {
                                                                    instance_clone.connect(&nm, ssid, WifiSecurity::Open).await;
                                                                } else {
                                                                    instance_clone.publish(NetworkState::Auth(ssid, AuthType::Password));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    Auth::Password(password) => instance_clone.connect(&nm, ssid, WifiSecurity::WpaPsk { psk: password }).await,
                                                }
                                            }
                                        }
                                    }
                                };

                                let _ = nm;
                            });
                        }
                        event = events.next() => {
                            match event {
                                Some(event_res) => {
                                    match event_res {
                                        Ok(event) => instance.handle_event(&nm, event, &mut debouncer).await,
                                        Err(err) => error!(&LOGGER, "Error: {err:?}"),
                                    }
                                }
                                None => break,
                            }
                        }
                        _ = &mut debouncer.sleep, if debouncer.deadline.is_some() => {
                            debouncer.deadline = None;
                            let _ = instance.send_all_networks(&nm).await;
                        }
                    }
                }
            });
        });
    }
}

impl NetworkBackendHandle {
    async fn connect(&self, nm: &NetworkManager, ssid: String, creds: WifiSecurity) {
        let previous_ssid = nm.current_ssid().await;
        let res = nm.connect(&ssid, None, creds).await;
        if res.is_err() {
            let err = res.err().unwrap();
            warn!(&LOGGER, "Error connecting to network {ssid}: {:?}", err);
            self.print_network_info(nm, &ssid, Level::WARN).await;
            self.publish(NetworkState::ConnectFail(ssid, ConnectError::from(err)));
            if let Some(prev) = previous_ssid {
                let _ = nm.connect(&prev, None, WifiSecurity::Open).await;
            }
            self.send_all_networks(nm).await;
        } else {
            self.publish(NetworkState::ConnectSuccess(ssid));
        }
    }

    async fn print_network_info(&self, nm: &NetworkManager, ssid: &str, level: Level) {
        if let Ok(nets) = nm.list_networks(None).await {
            if let Some(net) = nets.iter().find(|net| net.ssid.eq(ssid)) {
                log!(&LOGGER, level, "Network:");
                log!(&LOGGER, level, "  SSID: {}", net.ssid);
                log!(&LOGGER, level, "  Active: {}", net.is_active);
                log!(&LOGGER, level, "  Known: {}", net.known);
                log!(&LOGGER, level, "  Secured: {}", net.secured);
                if net.secured {
                    log!(&LOGGER, level, "    Features: {:?}", net.security_features);
                    log!(&LOGGER, level, "    EAP: {}", net.is_eap);
                    log!(&LOGGER, level, "    PSK: {}", net.is_psk);
                }
                log!(&LOGGER, level, "  Strength: {:?}", net.strength);
                log!(&LOGGER, level, "  Device: {}", net.device);
                log!(&LOGGER, level, "  Addr(4): {:?}", net.ip4_address);
                log!(&LOGGER, level, "  Addr(6): {:?}", net.ip6_address);
            }
        }
    }

    async fn handle_event(
        &self,
        nm: &NetworkManager,
        event: NetworkEvent,
        debouncer: &mut EventDebouncer,
    ) {
        match event {
            NetworkEvent::AccessPointsChanged => {
                let deadline = Instant::now() + debouncer.duration;
                debouncer.deadline = Some(deadline);
                debouncer.sleep.as_mut().reset(deadline);
            }
            NetworkEvent::WirelessEnabledChanged => {
                if let Ok(state) = nm.wifi_state().await {
                    if !state.enabled {
                        let _ = self.publish(NetworkState::ChangeNetwork(None));
                    } else {
                        let _ = self.send_active_connection(nm).await;
                    }
                }
            }
            NetworkEvent::ActiveConnectionsChanged => {
                let _ = self.send_active_connection(nm).await;
            }
            NetworkEvent::DeviceChanged { interface } => match interface {
                Some(interface) => match nm.active_vpn_connections().await {
                    Ok(connections) => {
                        let mut connected = false;
                        for conn in connections {
                            if conn.interface.is_some_and(|s| s.eq(&interface)) {
                                connected = true;
                            }
                        }
                        let _ = self.publish(NetworkState::VpnState {
                            profile: interface,
                            connected,
                        });
                    }
                    Err(err) => error!(&LOGGER, "Error getting active vpns: {}", err.to_string()),
                },
                None => {}
            },
            _ => {}
        }
    }

    async fn primary_connection(nm: &NetworkManager) -> Option<String> {
        let manager = zbus::fdo::PropertiesProxy::new(
            nm.dbus_connection(),
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
        )
        .await
        .ok()?;

        let path = OwnedObjectPath::try_from(
            manager
                .get(
                    InterfaceName::from_static_str("org.freedesktop.NetworkManager").unwrap(),
                    "PrimaryConnection",
                )
                .await
                .ok()?,
        )
        .ok()?;

        let props = zbus::fdo::PropertiesProxy::new(
            nm.dbus_connection(),
            "org.freedesktop.NetworkManager",
            path.as_str(),
        )
        .await
        .ok()?;

        let is_default = bool::try_from(
            props
                .get(
                    InterfaceName::from_static_str(
                        "org.freedesktop.NetworkManager.Connection.Active",
                    )
                    .unwrap(),
                    "Default",
                )
                .await
                .ok()?,
        )
        .ok()?;

        if is_default {
            let uuid: String = String::try_from(
                props
                    .get(
                        InterfaceName::from_static_str(
                            "org.freedesktop.NetworkManager.Connection.Active",
                        )
                        .unwrap(),
                        "Uuid",
                    )
                    .await
                    .ok()?,
            )
            .ok()?;
            return Some(uuid);
        }
        None
    }

    async fn send_all_networks(&self, nm: &NetworkManager) {
        let _ = nm.scan_networks(None).await;
        if let Ok(connections) = nm.list_access_points(None).await {
            let mut networks: Vec<NetworkInfo> = Vec::new();
            for conn in connections {
                if conn.is_hidden() {
                    continue;
                }
                let ssid = conn.ssid;
                let known = nm.has_saved_connection(&ssid).await.is_ok_and(|b| b);
                let network = NetworkInfo {
                    name: ssid,
                    network_type: NetworkType::Wifi,
                    signal: conn.strength,
                    secured: Some(!conn.security.is_open()),
                    known,
                };
                networks.push(network);
            }
            networks.sort();
            self.publish(NetworkState::Networks(networks));
        }
    }

    async fn send_active_connection(&self, nm: &NetworkManager) {
        trace!(&LOGGER, "Sending Active Connection");
        if let Ok(connections) = nm.list_active_connections().await {
            let connection = if let Some(primary) = Self::primary_connection(nm).await {
                connections.iter().find(|c| match c {
                    ActiveConnection::Wired(c) => c.uuid.eq(&primary),
                    ActiveConnection::Wifi(c) => c.uuid.eq(&primary),
                    _ => false,
                })
            } else {
                connections.first()
            };
            match connection {
                Some(conn) => {
                    match conn {
                        ActiveConnection::Wired(c) => {
                            let info = NetworkInfo {
                                name: c.id.clone(),
                                network_type: NetworkType::Wired,
                                signal: 0,
                                secured: None,
                                known: true,
                            };
                            let _ = self.publish(NetworkState::ChangeNetwork(Some(info)));
                        }
                        ActiveConnection::Wifi(c) => {
                            let info = NetworkInfo {
                                name: c.ssid.clone(),
                                network_type: NetworkType::Wifi,
                                signal: c.strength.unwrap_or(0),
                                secured: None, // Not relevant here
                                known: true,
                            };
                            let _ = self.publish(NetworkState::ChangeNetwork(Some(info)));
                        }
                        _ => {}
                    }
                }
                None => {
                    let _ = self.publish(NetworkState::ChangeNetwork(None));
                }
            }
        }
    }

    async fn toggle_vpn(&self, profile: String, nm: &NetworkManager) {
        let operation = async {
            let active = nm.active_vpn_connections().await?;
            if active.iter().any(|vpn| vpn.name == profile) {
                nm.disconnect_vpn(&profile).await?;
            } else {
                nm.connect_vpn_by_id(&profile).await?;
            };

            let active = nm.active_vpn_connections().await?;
            Ok::<bool, Box<dyn std::error::Error>>(active.iter().any(|vpn| vpn.name == profile))
        };

        match operation.await {
            Ok(connected) => {
                let _ = self.publish(NetworkState::VpnState { profile, connected });
            }
            Err(_) => {}
        }
    }
}
