use crate::LOGGER;
use crate::backend::{BackendThread, SimpleBackendThread};
use crossbeam_channel::{Receiver, Sender};
use hyprland::data::{Client, Monitor, Workspaces};
use hyprland::event_listener::EventListener;
use hyprland::prelude::*;
use hyprland::shared::{MonitorId, WorkspaceId, WorkspaceType};
use jmb_logger_rs::*;
use std::io;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum WindowManagerEvent {
    WorkspaceChanged(i32),
    WorkspaceAdded(i32, MonitorId),
    WorkspaceRemoved(i32),
    WindowFocusChanged(String),
    SubmapChange(Option<String>),
}

pub struct HyprlandComms {
    subscribers: Vec<Sender<WindowManagerEvent>>,
}

impl SimpleBackendThread<WindowManagerEvent> for HyprlandComms {

    fn subscribers(&self) -> &Vec<Sender<WindowManagerEvent>> {
        &self.subscribers
    }

    fn init(registration_rx: Receiver<Sender<WindowManagerEvent>>) {
        std::thread::spawn(move || {
            let instance = Arc::new(Mutex::new(Self {
                subscribers: vec![],
            }));
            let mut listener = EventListener::new();

            let inst_clone = instance.clone();
            listener.add_workspace_changed_handler(move |workspace| {
                if let WorkspaceType::Regular(name) = workspace.name
                    && let Ok(id) = name.parse::<i32>()
                {
                    inst_clone
                        .lock()
                        .expect("Couldn't get Subscribers")
                        .publish(WindowManagerEvent::WorkspaceChanged(id));
                }
            });

            let inst_clone = instance.clone();
            listener.add_workspace_added_handler(move |workspace| {
                if let WorkspaceType::Regular(name) = workspace.name {
                    //unpacks the name of the workspace
                    if let Ok(wss) = Workspaces::get() {
                        //gets all workspaces
                        if let Some(ws) = wss
                            .to_vec()
                            .iter()
                            .find(|ws| ws.id().unwrap() == workspace.id)
                        {
                            //find the current workspace Object from its id
                            if let Ok(id) = name.parse::<i32>() {
                                // parse the id as an int
                                inst_clone
                                    .lock()
                                    .expect("Couldn't get Subscribers")
                                    .publish(WindowManagerEvent::WorkspaceAdded(
                                        id,
                                        ws.monitor_id.unwrap(),
                                    ));
                            }
                        }
                    }
                }
            });

            let inst_clone = instance.clone();
            listener.add_workspace_deleted_handler(move |workspace| {
                if let WorkspaceType::Regular(name) = workspace.name
                    && let Ok(id) = name.parse::<i32>()
                {
                    inst_clone
                        .lock()
                        .expect("Couldn't get Subscribers")
                        .publish(WindowManagerEvent::WorkspaceRemoved(id));
                }
            });

            let inst_clone = instance.clone();
            listener.add_active_monitor_changed_v2_handler(move |evt| {
                inst_clone
                    .lock()
                    .expect("Couldn't get Subscribers")
                    .publish(WindowManagerEvent::WorkspaceChanged(evt.workspace_id));
            });

            let inst_clone = instance.clone();
            listener.add_active_window_changed_handler(move |data_opt| {
                let title = data_opt.map(|data| data.title).unwrap_or(String::new());
                inst_clone
                    .lock()
                    .expect("Couldn't get Subscribers")
                    .publish(WindowManagerEvent::WindowFocusChanged(title));
            });

            let inst_clone = instance.clone();
            listener.add_workspace_moved_handler(move |event| {
                if let Some(monitor_id) = Self::monitor_id_from_workspace_id(event.id) {
                    inst_clone
                        .lock()
                        .expect("Couldn't get Subscribers")
                        .publish(WindowManagerEvent::WorkspaceRemoved(event.id));
                    inst_clone
                        .lock()
                        .expect("Couldn't get Subscribers")
                        .publish(WindowManagerEvent::WorkspaceAdded(event.id, monitor_id));
                }
            });

            let inst_clone = instance.clone();
            listener.add_sub_map_changed_handler(move |submap| {
                let submap = if submap.is_empty() {
                    None
                } else {
                    Some(submap.clone())
                };
                inst_clone
                    .lock()
                    .expect("Couldn't get Subscribers")
                    .publish(WindowManagerEvent::SubmapChange(submap))
            });

            std::thread::spawn(move || {
                let inst_clone = instance.clone();
                while let Ok(tx) = registration_rx.recv() {
                    trace!(&LOGGER, "Added WM Hyprland Subscriber: {tx:?}");
                    inst_clone
                        .lock()
                        .expect("Couldn't get Subscribers")
                        .subscribers
                        .push(tx);
                }
            });

            if let Err(err) = listener.start_listener() {
                error!(&LOGGER, "Hyprland event listener failed {:?}", err);
            }
        });
    }
}

impl HyprlandComms {
    fn monitor_id_from_workspace_id(id: WorkspaceId) -> Option<MonitorId> {
        if let Ok(wss) = Workspaces::get()
            && let Some(ws) = wss.to_vec().iter().find(|ws| ws.id().unwrap() == id)
        {
            return ws.monitor_id;
        }
        None
    }

    pub fn _current_monitor() -> MonitorId {
        if let Ok(monitor) = Monitor::get_active() {
            return monitor.id;
        }
        0
    }

    pub fn workspace_added(id: WorkspaceId, monitor: Option<MonitorId>) -> bool {
        if let Ok(wss) = Workspaces::get() {
            wss.to_vec()
                .iter()
                .find(|ws| {
                    let b1 = ws.id().unwrap() == id;
                    let b2 =
                        monitor.is_some_and(|id| ws.monitor_id.is_some_and(|mon_id| id == mon_id));
                    b1 && b2
                })
                .is_some()
        } else {
            false
        }
    }

    pub fn get_current_title() -> String {
        if let Ok(Some(client)) = Client::get_active() {
            client.title
        } else {
            String::new()
        }
    }

    pub fn dispatch(dispatcher: &str) -> io::Result<Output> {
        Command::new("sh")
            .arg("-c")
            .arg(format!("hyprctl dispatch \"{dispatcher}\""))
            .output()
    }
}
