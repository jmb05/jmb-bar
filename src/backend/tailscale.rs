use crate::LOGGER;
use crate::backend::{BackendThread, Registrator};
use crossbeam_channel::{Receiver, Sender};
use http::Request;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use jmb_logger_rs::*;
use std::error::Error;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::net::UnixStream;

pub struct TailscaleBackend {
    subscribers: Vec<Sender<TailscaleState>>,
    state: TailscaleState,
}

#[derive(Debug, Clone)]
pub enum TailscaleState {
    NoState = 0,
    InUseOtherUser = 1,
    NeedsLogin = 2,
    NeedsMachineAuth = 3,
    Stopped = 4,
    Starting = 5,
    Running = 6,
}

impl TailscaleState {
    fn try_from_value(value: i64) -> Option<TailscaleState> {
        match value {
            0 => Some(TailscaleState::NoState),
            1 => Some(TailscaleState::InUseOtherUser),
            2 => Some(TailscaleState::NeedsLogin),
            3 => Some(TailscaleState::NeedsMachineAuth),
            4 => Some(TailscaleState::Stopped),
            5 => Some(TailscaleState::Starting),
            6 => Some(TailscaleState::Running),
            _ => None,
        }
    }
}

impl TailscaleBackend {
    async fn watch_tailscale(
        instance: Arc<Mutex<Self>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let socket = UnixStream::connect("/var/run/tailscale/tailscaled.sock").await?;
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
            .body(http_body_util::Empty::<hyper::body::Bytes>::new())?;

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
                                let state =
                                    TailscaleState::try_from_value(state.as_i64().unwrap_or(4))
                                        .unwrap_or(TailscaleState::Stopped);
                                instance.lock().expect("Couldn't get lock").publish(state);
                            }
                        }
                        Err(err) => {
                            eprintln!("Invalid Tailscale notification: {err}");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl BackendThread<TailscaleState, ()> for TailscaleBackend {
    fn subscribe(registrator: Registrator<TailscaleState>) -> (Receiver<TailscaleState>, ()) {
        let (tx, rx) = crossbeam_channel::unbounded::<TailscaleState>();
        let _ = registrator.send(tx);
        (rx, ())
    }

    fn subscribers(&self) -> &Vec<Sender<TailscaleState>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<TailscaleState>>) {
        let state = initial_state();

        let instance = Arc::new(Mutex::new(Self {
            subscribers: vec![],
            state,
        }));

        let instance_clone = instance.clone();
        std::thread::spawn(move || {
            while let Ok(tx) = registration_rx.recv() {
                trace!(&LOGGER, "Added Tailscale Subscriber: {tx:?}");

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
                    if let Err(e) = Self::watch_tailscale(instance.clone()).await {
                        warn!(&LOGGER, "Error: {e:?}");
                    }
                }
            });
        });
    }
}

fn initial_state() -> TailscaleState {
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
                return TailscaleState::Running;
            }
        } else {
            error!(&LOGGER, "{:?}", val.err());
        }
    }
    TailscaleState::Stopped
}
