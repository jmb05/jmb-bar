use crate::LOGGER;
use crate::backend::{BackendThread, Registrator};
use crossbeam_channel::{Receiver, Sender};
use http::Request;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper_util::rt::TokioIo;
use jmb_logger_rs::*;
use std::cmp::PartialEq;
use std::error::Error;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::net::UnixStream;

pub struct JsonSocketBackend {
    subscribers: Vec<Sender<State>>,
    state: State,
    socket_address: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Connected,
    Connecting,
    Disconnected,
    NeedsLogin,
}

impl State {
    fn _try_from_value(value: i64) -> Option<State> {
        match value {
            5 => Some(State::Connecting),
            6 => Some(State::Connected),
            _ => Some(State::Disconnected),
        }
    }
}

impl JsonSocketBackend {
    async fn _watch(instance: Arc<Mutex<Self>>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let socket_address = instance
            .lock()
            .expect("Couldn't get lock")
            .socket_address
            .clone();
        let socket = UnixStream::connect(&socket_address).await?;
        let io = TokioIo::new(socket);

        let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;

        tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!(&LOGGER, "Tailscale socket HTTP connection error: {err}");
            }
        });

        let request = Request::builder()
            .method("GET")
            .uri("http://local-tailscaled.sock/localapi/v0/watch-ipn-bus")
            .header("Host", "local-tailscaled.sock")
            .header("Accept", "application/json")
            .body(Empty::<Bytes>::new())?;

        let response = sender.send_request(request).await?;

        if !response.status().is_success() {
            return Err(format!("Tailscale API returned {}", response.status()).into());
        }

        let mut body: Incoming = response.into_body();
        let mut pending = Vec::<u8>::new();

        while let Some(frame_result) = body.frame().await {
            let frame = frame_result?;

            if let Ok(data) = frame.into_data() {
                pending.extend_from_slice(&data);

                while let Some(newline) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=newline).collect();
                    let line = &line[..line.len() - 1];

                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }

                    match serde_json::from_slice::<serde_json::Value>(line) {
                        Ok(notification) => {
                            if let Some(state) = notification.get("State")
                                && !state.is_null()
                            {
                                let state = State::_try_from_value(state.as_i64().unwrap_or(4))
                                    .unwrap_or(State::Disconnected);
                                instance.lock().expect("Couldn't get lock").publish(state);
                            }
                        }
                        Err(err) => {
                            warn!(&LOGGER, "Invalid Tailscale notification: {err}");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn watch_netbird(instance: Arc<Mutex<Self>>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let socket_address = instance
            .lock()
            .expect("Couldn't get lock")
            .socket_address
            .clone();

        let socket = UnixStream::connect(&socket_address).await?;
        let io = TokioIo::new(socket);

        let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;

        tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!(&LOGGER, "Netbird socket HTTP connection error: {err}");
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("http://localhost/daemon.DaemonService/SubscribeStatus")
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(Full::new(Bytes::from_static(b"{}")))?;

        let response = sender.send_request(request).await?;

        if !response.status().is_success() {
            return Err(format!("NetBird API returned {}", response.status()).into());
        }

        let mut body = response.into_body();
        let mut pending = Vec::<u8>::new();

        while let Some(frame_result) = body.frame().await {
            let frame = frame_result?;

            if let Ok(data) = frame.into_data() {
                pending.extend_from_slice(&data);

                while let Some(newline) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=newline).collect();
                    let line = &line[..line.len() - 1];

                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }

                    match serde_json::from_slice::<serde_json::Value>(line) {
                        Ok(notification) => {
                            let Some(status) = notification
                                .get("result")
                                .and_then(|v| v.get("status").and_then(|v| v.as_str()))
                            else {
                                continue;
                            };
                            let state = match status {
                                "Connected" => State::Connected,
                                "Connecting" => State::Connecting,
                                "Idle" => State::Disconnected,
                                "NeedsLogin" => State::NeedsLogin,
                                other => {
                                    warn!(&LOGGER, "Unknown NetBird status: {other}");
                                    continue;
                                }
                            };
                            instance.lock().expect("Couldn't get lock").publish(state);
                        }
                        Err(err) => {
                            warn!(&LOGGER, "Invalid NetBird notification: {err}");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl BackendThread<State, ()> for JsonSocketBackend {
    fn subscribe(registrator: Registrator<State>) -> (Receiver<State>, ()) {
        let (tx, rx) = crossbeam_channel::unbounded::<State>();
        let _ = registrator.send(tx);
        (rx, ())
    }

    fn subscribers(&self) -> &Vec<Sender<State>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<State>>) {
        let state = initial_state_tailscale();

        let instance = Arc::new(Mutex::new(Self {
            subscribers: vec![],
            state,
            socket_address: String::from("/var/run/netbird-http.sock"),
        }));

        let instance_clone = instance.clone();
        std::thread::spawn(move || {
            while let Ok(tx) = registration_rx.recv() {
                trace!(&LOGGER, "Added NetBird Subscriber: {tx:?}");

                let _ = tx.send(
                    instance_clone
                        .lock()
                        .expect("Couldn't get lock")
                        .state
                        .clone(),
                );

                instance_clone
                    .lock()
                    .expect("Couldn't get lock")
                    .subscribers
                    .push(tx);
            }
        });

        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            runtime.block_on(async move {
                loop {
                    if let Err(e) = Self::watch_netbird(instance.clone()).await {
                        warn!(&LOGGER, "Error: {e:?}");
                    }
                }
            });
        });
    }
}

fn initial_state_tailscale() -> State {
    if let Ok(output) = Command::new("tailscale")
        .arg("status")
        .arg("--json")
        .output()
    {
        let val: Result<serde_json::Value, _> = serde_json::from_slice(&output.stdout);
        if let Ok(val) = val {
            if let Some(res) = val.get("BackendState").and_then(|v| v.as_str())
                && res.eq("Running")
            {
                return State::Connected;
            }
        } else {
            error!(&LOGGER, "{:?}", val.err());
        }
    }
    State::Connecting
}
