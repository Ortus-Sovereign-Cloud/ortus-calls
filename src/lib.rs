//! Calls: signalling and the media server, audio, video and screen share.
//!
//! An Ortus app crate: it depends on `ortus-core` only and hands its apps
//! to a program through [`apps`]. A call belongs to a chat room; rooms
//! come through the core's Chat interface.

pub mod api;
pub mod node;
pub mod store;
pub mod turn;

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group, OutsideJobKind, RoleTask};
use ortus_core::config::Role;
use ortus_core::instance::Instance;
use ortus_core::storage::Migration;

/// Ends what is over: sessions not heard from, empty calls, gone nodes.
pub const SWEEP_JOB: &str = "calls.sweep";

/// The built web client (web/dist), served under /assets/apps/calls/.
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
struct Web;

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

    fn web_file(&self, path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
        Web::get(path).map(|file| file.data)
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn routes(&self) -> Vec<ortus_core::api::Route> {
        api::routes()
    }

    // The media node runs with the media role (one machine runs them all).
    fn role_tasks(&self) -> Vec<RoleTask> {
        vec![RoleTask {
            role: Role::Media,
            name: "media",
            run: node::run,
        }]
    }

    // A person forgotten: their notes go; their part in calls is anonymous.
    fn forget_user<'a>(
        &'a self,
        tx: &'a mut dyn ortus_core::storage::AsyncTx,
        user_id: &'a str,
        _rules: &'a ortus_core::retention::Rules,
    ) -> ortus_core::storage::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            for sql in [
                "DELETE FROM calls_notes WHERE user_id = $1",
                "UPDATE calls_sessions SET user_id = '' WHERE user_id = $1",
                "UPDATE calls_calls SET started_by = '' WHERE started_by = $1",
                "UPDATE calls_actions SET assignee = '' WHERE assignee = $1",
                "UPDATE calls_actions SET made_by = '' WHERE made_by = $1",
            ] {
                tx.execute(sql, &[user_id.into()])
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }

    fn periodic(&self) -> Vec<(&'static str, i64)> {
        vec![(SWEEP_JOB, 10_000)]
    }

    fn outside_jobs(&self, instance: &Arc<Instance>) -> Vec<OutsideJobKind> {
        let instance = Arc::clone(instance);
        vec![OutsideJobKind {
            kind: SWEEP_JOB.into(),
            handler: Box::new(move |_| {
                let instance = Arc::clone(&instance);
                Box::pin(async move { sweep(&instance).await })
            }),
        }]
    }
}

/// Sessions not heard from leave, empty calls end, and calls whose media
/// node has gone end.
pub async fn sweep(instance: &Instance) -> Result<(), String> {
    let s = instance.store.as_ref();
    let stale = store::now() - store::STALE_MS;
    let rows = s
        .query("SELECT s.id, s.call FROM calls_sessions s JOIN calls_calls c ON c.id = s.call WHERE s.left_at IS NULL AND (s.seen_at <= $1 OR c.ended_at IS NOT NULL) LIMIT 500", &[stale.into()])
        .await
        .map_err(|e| e.to_string())?;
    for r in rows {
        let (session, call) = (
            r.text(0).unwrap_or_default().to_string(),
            r.text(1).unwrap_or_default().to_string(),
        );
        if let Ok(Some(c)) = store::call(s, &call).await {
            api::left(instance, &session, &c).await;
        }
    }
    let gone = store::now() - store::NODE_STALE_MS;
    s.execute("UPDATE calls_calls SET ended_at = $1 WHERE ended_at IS NULL AND node NOT IN (SELECT id FROM calls_nodes WHERE seen_at > $2)", &[store::now().into(), gone.into()])
        .await
        .map_err(|e| e.to_string())?;
    s.execute(
        "DELETE FROM calls_nodes WHERE seen_at <= $1",
        &[(gone - 3_600_000).into()],
    )
    .await
    .map_err(|e| e.to_string())?;
    // Calls left open with nobody in them.
    let open = s
        .query("SELECT id FROM calls_calls WHERE ended_at IS NULL", &[])
        .await
        .map_err(|e| e.to_string())?;
    for r in open {
        let _ = store::end_if_empty(s, r.text(0).unwrap_or_default()).await;
    }
    Ok(())
}

/// The apps of this crate, for a program to run.
pub fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Calls)]
}
