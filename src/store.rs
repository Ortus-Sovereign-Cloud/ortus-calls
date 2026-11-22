//! Calls' tables: media nodes, calls and who is in them.
//!
//! A call belongs to a chat room (a channel's call); a room has at most one
//! call going at a time. Each person in it has a session (one browser or
//! phone), with what they show: muted, camera, screen, a raised hand.

use ortus_core::storage::{AsyncStore, AsyncTx, Migration, StoreResult};
use serde_json::{json, Value};

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "media nodes, calls, sessions",
        sql: "CREATE TABLE calls_nodes (
        id TEXT PRIMARY KEY,
        signal TEXT NOT NULL,
        token_sealed TEXT NOT NULL,
        seen_at BIGINT NOT NULL
    );
    CREATE TABLE calls_calls (
        id TEXT PRIMARY KEY,
        room TEXT NOT NULL,
        node TEXT NOT NULL,
        started_by TEXT NOT NULL,
        started_at BIGINT NOT NULL,
        ended_at BIGINT
    );
    CREATE INDEX calls_calls_room ON calls_calls (room, ended_at);
    CREATE TABLE calls_sessions (
        id TEXT PRIMARY KEY,
        call TEXT NOT NULL,
        user_id TEXT NOT NULL,
        joined_at BIGINT NOT NULL,
        seen_at BIGINT NOT NULL,
        left_at BIGINT,
        muted BIGINT NOT NULL,
        camera BIGINT NOT NULL,
        screen TEXT,
        hand BIGINT
    );
    CREATE INDEX calls_sessions_call ON calls_sessions (call, left_at)",
    },
    Migration {
        version: 2,
        name: "a document presented, live notes, action items",
        sql: "ALTER TABLE calls_sessions ADD COLUMN doc TEXT;
    CREATE TABLE calls_notes (
        id TEXT PRIMARY KEY,
        call TEXT NOT NULL,
        user_id TEXT NOT NULL,
        text TEXT NOT NULL,
        at BIGINT NOT NULL
    );
    CREATE INDEX calls_notes_call ON calls_notes (call, at);
    CREATE TABLE calls_actions (
        id TEXT PRIMARY KEY,
        call TEXT NOT NULL,
        title TEXT NOT NULL,
        assignee TEXT NOT NULL,
        due BIGINT,
        task_path TEXT NOT NULL,
        made_by TEXT NOT NULL,
        at BIGINT NOT NULL
    );
    CREATE INDEX calls_actions_call ON calls_actions (call, at)",
    },
    Migration {
        version: 3,
        name: "one live call a room",
        sql: "UPDATE calls_calls SET ended_at = started_at WHERE ended_at IS NULL AND EXISTS (SELECT 1 FROM calls_calls c WHERE c.room = calls_calls.room AND c.ended_at IS NULL AND (c.started_at > calls_calls.started_at OR (c.started_at = calls_calls.started_at AND c.id > calls_calls.id)));
    CREATE UNIQUE INDEX calls_calls_live ON calls_calls (room) WHERE ended_at IS NULL",
    },
    // Where browsers reach the node's TURN relay (a turns: URL), if it has one.
    Migration {
        version: 4,
        name: "TURN relay",
        sql: "ALTER TABLE calls_nodes ADD COLUMN turn TEXT",
    },
];

/// A node's TURN relay URL and its token (sealed), when it has a relay.
pub async fn node_turn(s: &dyn AsyncStore, id: &str) -> StoreResult<Option<(String, String)>> {
    Ok(s.query(
        "SELECT turn, token_sealed FROM calls_nodes WHERE id = $1 AND turn IS NOT NULL",
        &[id.into()],
    )
    .await?
    .first()
    .map(|r| {
        (
            r.text(0).unwrap_or_default().to_string(),
            r.text(1).unwrap_or_default().to_string(),
        )
    }))
}

/// A session not heard from for this long has left.
pub const STALE_MS: i64 = 30_000;
/// A media node not heard from for this long is gone.
pub const NODE_STALE_MS: i64 = 30_000;
/// At most this many people in a call (small calls first).
pub const MAX_PEOPLE: usize = 25;

pub fn now() -> i64 {
    ortus_core::program::now_ms()
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", ortus_core::auth::random_hex(12))
}

#[derive(Clone, Debug)]
pub struct Call {
    pub id: String,
    pub room: String,
    pub node: String,
    pub started_by: String,
    pub started_at: i64,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub call: String,
    pub user: String,
    pub joined_at: i64,
    pub muted: bool,
    pub camera: bool,
    pub screen: Option<String>,
    pub hand: Option<i64>,
    /// The document this person presents (a Docs document id).
    pub doc: Option<String>,
}

impl Session {
    pub fn json(&self) -> Value {
        json!({"session": self.id, "user": self.user, "joinedAt": self.joined_at, "muted": self.muted, "camera": self.camera, "screen": self.screen, "hand": self.hand, "doc": self.doc})
    }
}

const CALL: &str = "SELECT id, room, node, started_by, started_at FROM calls_calls";

fn call_of(r: &ortus_core::storage::Row) -> Call {
    Call {
        id: r.text(0).unwrap_or_default().into(),
        room: r.text(1).unwrap_or_default().into(),
        node: r.text(2).unwrap_or_default().into(),
        started_by: r.text(3).unwrap_or_default().into(),
        started_at: r.int(4).unwrap_or(0),
    }
}

/// The call going on in a room, if any.
pub async fn live_call(s: &dyn AsyncStore, room: &str) -> StoreResult<Option<Call>> {
    Ok(s.query(
        &format!("{CALL} WHERE room = $1 AND ended_at IS NULL ORDER BY started_at LIMIT 1"),
        &[room.into()],
    )
    .await?
    .first()
    .map(call_of))
}

/// Every call going on, the newest first.
pub async fn live_calls(s: &dyn AsyncStore) -> StoreResult<Vec<Call>> {
    Ok(s.query(
        &format!("{CALL} WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 200"),
        &[],
    )
    .await?
    .iter()
    .map(call_of)
    .collect())
}

pub async fn call(s: &dyn AsyncStore, id: &str) -> StoreResult<Option<Call>> {
    Ok(s.query(&format!("{CALL} WHERE id = $1"), &[id.into()])
        .await?
        .first()
        .map(call_of))
}

const SESSION: &str =
    "SELECT id, call, user_id, joined_at, muted, camera, screen, hand, doc FROM calls_sessions";

fn session_of(r: &ortus_core::storage::Row) -> Session {
    Session {
        id: r.text(0).unwrap_or_default().into(),
        call: r.text(1).unwrap_or_default().into(),
        user: r.text(2).unwrap_or_default().into(),
        joined_at: r.int(3).unwrap_or(0),
        muted: r.int(4).unwrap_or(0) != 0,
        camera: r.int(5).unwrap_or(0) != 0,
        screen: r.text(6).map(str::to_string),
        hand: r.int(7),
        doc: r.text(8).map(str::to_string),
    }
}

/// Who is in a call now, in the order they came.
pub async fn sessions(s: &dyn AsyncStore, call: &str) -> StoreResult<Vec<Session>> {
    Ok(s.query(
        &format!(
            "{SESSION} WHERE call = $1 AND left_at IS NULL AND seen_at > $2 ORDER BY joined_at, id"
        ),
        &[call.into(), (now() - STALE_MS).into()],
    )
    .await?
    .iter()
    .map(session_of)
    .collect())
}

pub async fn session(s: &dyn AsyncStore, id: &str) -> StoreResult<Option<Session>> {
    Ok(s.query(
        &format!("{SESSION} WHERE id = $1 AND left_at IS NULL"),
        &[id.into()],
    )
    .await?
    .first()
    .map(session_of))
}

/// Starts a call in a room, or answers the one going on (one at a time:
/// the room's open call is looked for inside the transaction).
pub async fn start_or_join(
    tx: &mut dyn AsyncTx,
    room: &str,
    node: &str,
    user: &str,
) -> StoreResult<(Call, bool)> {
    if let Some(r) = tx
        .query(
            &format!("{CALL} WHERE room = $1 AND ended_at IS NULL ORDER BY started_at LIMIT 1"),
            &[room.into()],
        )
        .await?
        .first()
    {
        return Ok((call_of(r), false));
    }
    let call = Call {
        id: new_id("c"),
        room: room.into(),
        node: node.into(),
        started_by: user.into(),
        started_at: now(),
    };
    tx.execute(
        "INSERT INTO calls_calls (id, room, node, started_by, started_at) VALUES ($1, $2, $3, $4, $5)",
        &[call.id.as_str().into(), room.into(), node.into(), user.into(), call.started_at.into()],
    )
    .await?;
    Ok((call, true))
}

pub async fn add_session(tx: &mut dyn AsyncTx, call: &str, user: &str) -> StoreResult<Session> {
    let s = Session {
        id: new_id("s"),
        call: call.into(),
        user: user.into(),
        joined_at: now(),
        muted: false,
        camera: true,
        screen: None,
        hand: None,
        doc: None,
    };
    tx.execute(
        "INSERT INTO calls_sessions (id, call, user_id, joined_at, seen_at, muted, camera) VALUES ($1, $2, $3, $4, $4, 0, 1)",
        &[s.id.as_str().into(), call.into(), user.into(), s.joined_at.into()],
    )
    .await?;
    Ok(s)
}

/// A call with nobody left in it ends.
pub async fn end_if_empty(s: &dyn AsyncStore, call: &str) -> StoreResult<bool> {
    // One statement: someone joining meanwhile keeps the call going.
    let n = s
        .execute(
            "UPDATE calls_calls SET ended_at = $1 WHERE id = $2 AND ended_at IS NULL AND NOT EXISTS (SELECT 1 FROM calls_sessions WHERE call = $2 AND left_at IS NULL AND seen_at > $3)",
            &[now().into(), call.into(), (now() - STALE_MS).into()],
        )
        .await?;
    Ok(n == 1)
}

/// A live media node to put a new call on (the one with fewest calls).
pub async fn pick_node(s: &dyn AsyncStore) -> StoreResult<Option<String>> {
    let rows = s
        .query(
            "SELECT n.id, (SELECT COUNT(*) FROM calls_calls c WHERE c.node = n.id AND c.ended_at IS NULL) AS load FROM calls_nodes n WHERE n.seen_at > $1 ORDER BY load, n.id LIMIT 1",
            &[(now() - NODE_STALE_MS).into()],
        )
        .await?;
    Ok(rows.first().and_then(|r| r.text(0).map(str::to_string)))
}

/// Where a node takes signalling, and its token (sealed).
pub async fn node(s: &dyn AsyncStore, id: &str) -> StoreResult<Option<(String, String)>> {
    Ok(s.query(
        "SELECT signal, token_sealed FROM calls_nodes WHERE id = $1",
        &[id.into()],
    )
    .await?
    .first()
    .map(|r| {
        (
            r.text(0).unwrap_or_default().to_string(),
            r.text(1).unwrap_or_default().to_string(),
        )
    }))
}

/// A call's live notes (the latest), oldest first.
pub async fn notes(s: &dyn AsyncStore, call: &str) -> StoreResult<Vec<Value>> {
    let rows = s.query("SELECT id, user_id, text, at FROM calls_notes WHERE call = $1 ORDER BY at DESC, id DESC LIMIT 50", &[call.into()]).await?;
    let mut out: Vec<Value> = rows
        .iter()
        .map(|r| json!({"id": r.text(0), "user": r.text(1), "text": r.text(2), "at": r.int(3)}))
        .collect();
    out.reverse();
    Ok(out)
}

/// A call's action items (tasks made in it).
pub async fn actions(s: &dyn AsyncStore, call: &str) -> StoreResult<Vec<Value>> {
    let rows = s.query("SELECT id, title, assignee, due, task_path FROM calls_actions WHERE call = $1 ORDER BY at, id LIMIT 100", &[call.into()]).await?;
    Ok(rows.iter().map(|r| json!({"id": r.text(0), "title": r.text(1), "assignee": r.text(2), "due": r.int(3), "path": r.text(4)})).collect())
}
