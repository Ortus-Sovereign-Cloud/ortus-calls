//! Calls: signalling and the media server, audio, video and screen share.
//!
//! An Ortus app crate: it depends on `ortus-core` only and hands its apps
//! to a program through [`apps`].

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};

/// The calls app.
pub struct Calls;

impl App for Calls {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "calls",
            group: Group::Communicate,
            path: "/calls",
            order: 3,
        }
    }
}

/// The apps of this crate, for a program to run.
pub fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Calls)]
}
