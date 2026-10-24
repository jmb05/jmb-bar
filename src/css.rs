use glib::ControlFlow;
use jmb_logger_rs::*;
use notify::EventKind::{Create, Modify, Remove};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use crate::LOGGER;

pub fn parse_css() -> Option<String> {
    match grass::from_path(
        "style/style.scss",
        &grass::Options::default(),
    ) {
        Ok(css) => {
            debug!(&LOGGER, "(Re-)Loading CSS");
            Some(css)
        },
        Err(err) => {
            error!(&LOGGER, "SCSS compilation failed: {err}");
            None
        },
    }
}

pub fn watch_css<F>(reload: F) -> RecommendedWatcher
where F: Fn(String) -> () + 'static {
    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(tx).unwrap();

    watcher
        .watch(
            Path::new("style"),
            RecursiveMode::Recursive,
        )
        .unwrap();

    glib::timeout_add_local(Duration::from_millis(1000), move || {
        let mut changed = false;
        while let Ok(Ok(event)) = rx.try_recv() {
            match event.kind {
                Create(_) | Modify(_) | Remove(_) => changed = true,
                _ => {}
            }
        }

        if changed && let Some(css) = parse_css() {
            reload(css);
        }

        ControlFlow::Continue
    });

    watcher
}