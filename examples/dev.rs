//! The core with this crate's apps, for development and the browser tests
//! (`cargo run --example dev`), with a stand-in for chat's rooms: anyone
//! signed in is in any room, named after its id. The released programs
//! (with the real chat) are built in the `ortus` repository.

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::instance::Instance;
use ortus_core::interfaces::{Chat, ChatRoom, ItemRef};
use ortus_core::storage::BoxFuture;

struct DevChat;

impl Chat for DevChat {
    fn post<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<ItemRef, String>> {
        Box::pin(async { Err("no chat here".into()) })
    }

    fn room<'a>(
        &'a self,
        user: &'a str,
        room: &'a str,
    ) -> BoxFuture<'a, Result<Option<ChatRoom>, String>> {
        Box::pin(async move {
            Ok(Some(ChatRoom {
                id: room.into(),
                name: room.into(),
                kind: "channel".into(),
                encrypted: false,
                members: vec![user.into()],
            }))
        })
    }

    fn notice<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// Provides the stand-in.
struct DevRooms;

impl App for DevRooms {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "chat",
            group: Group::Communicate,
            path: "/chat",
            order: 1,
        }
    }

    fn provide(&self, _: &Arc<Instance>, services: &ortus_core::services::Services) {
        let chat: Arc<dyn Chat> = Arc::new(DevChat);
        services.provide("chat", chat);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut apps = ortus_calls::apps();
    apps.push(Arc::new(DevRooms));
    let status = ortus_core::cli::run("ortus-calls-dev", ortus_core::VERSION, apps, &args).await;
    std::process::exit(status);
}
