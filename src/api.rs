//! The calls API: a room's call (who is in it), joining it, handing the
//! browser's offer to the call's media node, saying what one shows (muted,
//! camera, screen, a raised hand; also the heartbeat), and leaving.
//! Rooms are chat's: only people in a room see or join its call.

use std::sync::Arc;

use ortus_core::api::{answer, body, ok, ApiError, Method, Route};
use ortus_core::auth::accounts::User;
use ortus_core::auth::Access;
use ortus_core::instance::Instance;
use ortus_core::interfaces::{Chat, ChatRoom};
use ortus_core::storage::BoxFuture;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::store::{self, Call};

async fn signed(request: &Request, instance: &Arc<Instance>) -> Result<User, ApiError> {
    Ok(ortus_core::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user)
}

async fn chat(instance: &Instance) -> Result<Arc<dyn Chat>, ApiError> {
    instance
        .services
        .get::<dyn Chat>(&instance.switches)
        .await
        .map_err(|_| ApiError::not_installed("chat"))
}

/// The room, when the person is in it.
async fn room_of(instance: &Instance, user: &str, room: &str) -> Result<ChatRoom, ApiError> {
    chat(instance)
        .await?
        .room(user, room)
        .await
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| ApiError::not_found("There is no such room."))
}

/// A call as the screen shows it: who is in it, by name.
async fn call_json(instance: &Instance, call: &Call) -> Result<Value, ApiError> {
    let s = instance.store.as_ref();
    let sessions = store::sessions(s, &call.id).await?;
    let notes = store::notes(s, &call.id).await?;
    let actions = store::actions(s, &call.id).await?;
    let mut people = serde_json::Map::new();
    let who = sessions.iter().map(|x| x.user.clone()).chain(
        notes.iter().chain(actions.iter()).filter_map(|v| {
            v["user"]
                .as_str()
                .or(v["assignee"].as_str())
                .map(str::to_string)
        }),
    );
    for user in who {
        if !people.contains_key(&user) {
            let name = ortus_core::auth::accounts::by_id(s, &user)
                .await?
                .map(|u| u.name)
                .unwrap_or_default();
            people.insert(user, json!({"name": name}));
        }
    }
    Ok(
        json!({"id": call.id, "room": call.room, "startedAt": call.started_at, "sessions": sessions.iter().map(store::Session::json).collect::<Vec<_>>(), "people": people, "notes": notes, "actions": actions}),
    )
}

async fn get_room(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let room = room_of(instance, &me.id, params.get("room").unwrap_or_default()).await?;
    let call = match store::live_call(instance.store.as_ref(), &room.id).await? {
        Some(c) => call_json(instance, &c).await?,
        None => Value::Null,
    };
    Ok(ok(
        &json!({"call": call, "encrypted": room.encrypted, "name": room.name, "kind": room.kind}),
    ))
}

/// The calls going on in rooms the person is in: each room's name and
/// kind, when it started, and who is in it (for the Calls page and the
/// start page's card).
async fn live(
    request: &Request,
    _: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let s = instance.store.as_ref();
    let chat = chat(instance).await?;
    let mut live = Vec::new();
    for call in store::live_calls(s).await? {
        let Ok(Some(room)) = chat.room(&me.id, &call.room).await else {
            continue;
        };
        let sessions = store::sessions(s, &call.id).await?;
        if sessions.is_empty() {
            continue;
        }
        let mut people = Vec::new();
        for user in sessions.iter().map(|x| x.user.as_str()) {
            if people.iter().any(|(id, _): &(String, String)| id == user) {
                continue;
            }
            let name = ortus_core::auth::accounts::by_id(s, user)
                .await?
                .map(|u| u.name)
                .unwrap_or_default();
            people.push((user.to_string(), name));
        }
        live.push(json!({
            "room": room.id, "name": room.name, "kind": room.kind, "startedAt": call.started_at,
            "people": people.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        }));
    }
    Ok(ok(&json!({ "live": live })))
}

/// Joins the room's call, starting it when there is none.
async fn join(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let room = room_of(instance, &me.id, params.get("room").unwrap_or_default()).await?;
    if room.encrypted {
        // The media node would hear what the room keeps from the server.
        return Err(ApiError::conflict(
            "Calls are not end to end yet, so an end-to-end room has none.",
        ));
    }
    let s = instance.store.as_ref();
    let node = store::pick_node(s).await?.ok_or_else(|| ApiError {
        status: 409,
        code: "no_media",
        message: "No media server is running.".into(),
    })?;
    // Two first joins at once (PostgreSQL): the unique index lets one
    // start the call; the other tries again and joins it.
    let mut attempt = 0;
    let (mut tx, call, started) = loop {
        let mut tx = s.begin().await?;
        match store::start_or_join(tx.as_mut(), &room.id, &node, &me.id).await {
            Ok((call, started)) => break (tx, call, started),
            Err(e) => {
                let _ = tx.rollback().await;
                attempt += 1;
                if attempt >= 3 {
                    return Err(e.into());
                }
            }
        }
    };
    // Counted inside the transaction (a store query while it is open
    // would wait for it on SQLite).
    let present = tx
        .query(
            "SELECT COUNT(*) FROM calls_sessions WHERE call = $1 AND left_at IS NULL AND seen_at > $2",
            &[call.id.as_str().into(), (store::now() - store::STALE_MS).into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if !started && present >= store::MAX_PEOPLE as i64 {
        return Err(ApiError::conflict("The call is full."));
    }
    // One person takes at most three places (a laptop, a phone, a tablet),
    // so nobody fills a call alone.
    let mine = tx
        .query(
            "SELECT COUNT(*) FROM calls_sessions WHERE call = $1 AND user_id = $2 AND left_at IS NULL AND seen_at > $3",
            &[call.id.as_str().into(), me.id.as_str().into(), (store::now() - store::STALE_MS).into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if mine >= 3 {
        return Err(ApiError::conflict(
            "You are in this call on three devices already.",
        ));
    }
    let session = store::add_session(tx.as_mut(), &call.id, &me.id).await?;
    tx.commit().await?;
    if started {
        // The room hears of it (its screens show a call to join).
        let _ = chat(instance)
            .await?
            .notice(&me.id, &room.id, "ortus.call", &call.id)
            .await;
        // And each other member, as a notification their phones ring for
        // (the core sends a call's notification as a call: CallKit on an
        // iPhone, a full-screen call on Android).
        let room_name = if room.kind == "channel" {
            format!("#{}", room.name)
        } else {
            room.name.clone()
        };
        for member in room.members.iter().filter(|m| **m != me.id) {
            let _ = ortus_core::notify::add(
                instance.store.as_ref(),
                member,
                &ortus_core::notify::Notice {
                    app: "calls",
                    kind: ortus_core::notify::Kind::Alert,
                    title: &format!("{} started a call in {room_name}", me.name),
                    detail: "",
                    path: &format!("/calls?room={}", room.id),
                    key: Some("callStarted"),
                    args: json!({"who": me.name, "room": room_name}),
                },
            )
            .await;
        }
        ortus_core::auth::audit(
            instance,
            ortus_core::audit::Event::new("calls.started", ortus_core::audit::Outcome::Done)
                .by(&me.id)
                .on(&room.id)
                .with(json!({"call": call.id})),
        )
        .await;
    }
    // The node's TURN relay, with credentials for this session: for
    // networks where only web ports get through. They last as long as a
    // long meeting (each relayed message checks them), not a day.
    let ice_servers = match store::node_turn(s, &call.node).await? {
        Some((url, sealed)) => match instance.keys.open_text(&sealed) {
            Ok(token) => {
                let (username, credential) = crate::turn::credentials(
                    &token,
                    &session.id,
                    (store::now() / 1000) as u64,
                    crate::turn::CREDENTIAL_SECS,
                );
                json!([{"urls": [url], "username": username, "credential": credential}])
            }
            Err(_) => json!([]),
        },
        None => json!([]),
    };
    Ok(ok(
        &json!({"call": call_json(instance, &call).await?, "session": session.id, "ice_servers": ice_servers}),
    ))
}

/// One's own session in a live call.
async fn my_session(
    instance: &Instance,
    me: &User,
    id: &str,
) -> Result<(store::Session, Call), ApiError> {
    let s = instance.store.as_ref();
    let session = store::session(s, id)
        .await?
        .filter(|x| x.user == me.id)
        .ok_or_else(|| ApiError::not_found("There is no such call."))?;
    let call = store::call(s, &session.call)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such call."))?;
    // Still in the room: someone taken out of it is out of its call (their
    // heartbeat fails, and the sweep ends their part).
    room_of(instance, &me.id, &call.room).await?;
    Ok((session, call))
}

/// A request to a media node: plain HTTP on the inside network, signed
/// with the node's token.
async fn to_node(
    instance: &Instance,
    node: &str,
    path: &str,
    body: &Value,
) -> Result<Value, ApiError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (signal, sealed) = store::node(instance.store.as_ref(), node)
        .await?
        .ok_or_else(|| ApiError {
            status: 409,
            code: "no_media",
            message: "The call's media server has gone.".into(),
        })?;
    let token = instance
        .keys
        .open_text(&sealed)
        .map_err(|_| ApiError::internal())?;
    let address = signal
        .strip_prefix("http://")
        .ok_or_else(ApiError::internal)?
        .trim_end_matches('/')
        .to_string();
    let text = body.to_string();
    let request = format!("POST {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
    let exchange = async {
        let mut conn = tokio::net::TcpStream::connect(&address).await?;
        conn.write_all(request.as_bytes()).await?;
        let mut out = Vec::new();
        conn.take(256 * 1024).read_to_end(&mut out).await?;
        Ok::<_, std::io::Error>(out)
    };
    let raw = tokio::time::timeout(std::time::Duration::from_secs(10), exchange)
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError {
            status: 409,
            code: "no_media",
            message: "The call's media server does not answer.".into(),
        })?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, rest) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let value: Value = serde_json::from_str(rest).unwrap_or(Value::Null);
    if !head.starts_with("HTTP/1.1 200") {
        return Err(ApiError::bad_request(
            value["error"]
                .as_str()
                .unwrap_or("The media server refused."),
        ));
    }
    Ok(value)
}

#[derive(Deserialize)]
struct Offer {
    offer: Value,
}

async fn offer(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (session, call) =
        my_session(instance, &me, params.get("session").unwrap_or_default()).await?;
    let input: Offer = body(request)?;
    let out = to_node(
        instance,
        &call.node,
        "/offer",
        &json!({"call": call.id, "session": session.id, "offer": input.offer}),
    )
    .await?;
    Ok(ok(&json!({"answer": out["answer"]})))
}

#[derive(Deserialize)]
struct State {
    muted: Option<bool>,
    camera: Option<bool>,
    /// The browser's own mid of its screen track, or null when not sharing.
    #[serde(default, with = "double_option")]
    screen: Option<Option<String>>,
    hand: Option<bool>,
    /// The document one presents (a Docs id), or null to stop.
    #[serde(default, with = "double_option")]
    doc: Option<Option<String>>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Ok(Some(Option::<String>::deserialize(d)?))
    }
}

/// What one shows; also says one is still there.
async fn state(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (session, call) =
        my_session(instance, &me, params.get("session").unwrap_or_default()).await?;
    let input: State = body(request)?;
    let s = instance.store.as_ref();
    let mut tx = s.begin().await?;
    tx.execute(
        "UPDATE calls_sessions SET seen_at = $1 WHERE id = $2",
        &[store::now().into(), session.id.as_str().into()],
    )
    .await?;
    if let Some(m) = input.muted {
        tx.execute(
            "UPDATE calls_sessions SET muted = $1 WHERE id = $2",
            &[i64::from(m).into(), session.id.as_str().into()],
        )
        .await?;
    }
    if let Some(c) = input.camera {
        tx.execute(
            "UPDATE calls_sessions SET camera = $1 WHERE id = $2",
            &[i64::from(c).into(), session.id.as_str().into()],
        )
        .await?;
    }
    if let Some(screen) = input.screen {
        let screen = screen.filter(|m| {
            !m.is_empty()
                && m.len() <= 32
                && m.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
        tx.execute(
            "UPDATE calls_sessions SET screen = $1 WHERE id = $2",
            &[screen.into(), session.id.as_str().into()],
        )
        .await?;
    }
    if let Some(doc) = input.doc {
        let doc = doc.filter(|d| {
            !d.is_empty()
                && d.len() <= 64
                && d.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
        // One presents a document or a screen, not both.
        tx.execute(
            "UPDATE calls_sessions SET doc = $1 WHERE id = $2",
            &[doc.into(), session.id.as_str().into()],
        )
        .await?;
    }
    if let Some(h) = input.hand {
        let at: Option<i64> = h.then(store::now);
        tx.execute(
            "UPDATE calls_sessions SET hand = $1 WHERE id = $2",
            &[at.into(), session.id.as_str().into()],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(ok(&json!({"call": call_json(instance, &call).await?})))
}

async fn leave(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (session, call) =
        my_session(instance, &me, params.get("session").unwrap_or_default()).await?;
    left(instance, &session.id, &call).await;
    Ok(ok(&json!({"left": session.id})))
}

/// A session leaves: its media stops, and an empty call ends.
pub async fn left(instance: &Instance, session: &str, call: &Call) {
    let s = instance.store.as_ref();
    let _ = s
        .execute(
            "UPDATE calls_sessions SET left_at = $1 WHERE id = $2 AND left_at IS NULL",
            &[store::now().into(), session.into()],
        )
        .await;
    let _ = to_node(instance, &call.node, "/kick", &json!({"session": session})).await;
    if store::end_if_empty(s, &call.id).await.unwrap_or(false) {
        if let Ok(chat) = chat(instance).await {
            let _ = chat
                .notice(&call.started_by, &call.room, "ortus.call", &call.id)
                .await;
        }
    }
}

#[derive(Deserialize)]
struct Note {
    text: String,
}

/// A line in the call's live notes.
async fn note(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (_, call) = my_session(instance, &me, params.get("session").unwrap_or_default()).await?;
    let input: Note = body(request)?;
    let text: String = input
        .text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(1000)
        .collect();
    if text.is_empty() {
        return Err(ApiError::bad_request("Write something."));
    }
    let s = instance.store.as_ref();
    let count = s
        .query(
            "SELECT COUNT(*) FROM calls_notes WHERE call = $1",
            &[call.id.as_str().into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if count >= 1000 {
        return Err(ApiError::conflict("The call has too many notes."));
    }
    s.execute(
        "INSERT INTO calls_notes (id, call, user_id, text, at) VALUES ($1, $2, $3, $4, $5)",
        &[
            store::new_id("t").into(),
            call.id.as_str().into(),
            me.id.as_str().into(),
            text.into(),
            store::now().into(),
        ],
    )
    .await?;
    Ok(ok(&json!({"call": call_json(instance, &call).await?})))
}

#[derive(Deserialize)]
struct Action {
    title: String,
    /// Who does it (a person in the call, by user id).
    assignee: String,
    /// When it is due (Unix ms).
    due: Option<i64>,
}

/// An action item: a task (through the tasks app) for someone in the call.
async fn action(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (_, call) = my_session(instance, &me, params.get("session").unwrap_or_default()).await?;
    let input: Action = body(request)?;
    let title: String = input
        .title
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    if title.is_empty() {
        return Err(ApiError::bad_request("Say what is to be done."));
    }
    let s = instance.store.as_ref();
    let made = s
        .query(
            "SELECT COUNT(*) FROM calls_actions WHERE call = $1",
            &[call.id.as_str().into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if made >= 200 {
        return Err(ApiError::conflict("The call has too many action items."));
    }
    let room = room_of(instance, &me.id, &call.room).await?;
    if !room.members.contains(&input.assignee) {
        return Err(ApiError::bad_request(
            "The task goes to someone in the room.",
        ));
    }
    let tasks = instance
        .services
        .get::<dyn ortus_core::interfaces::Tasks>(&instance.switches)
        .await
        .map_err(|_| ApiError::not_installed("tasks"))?;
    let from = ortus_core::interfaces::ItemRef {
        app: "calls".into(),
        id: call.id.clone(),
        path: format!("/calls?room={}", call.room),
    };
    let task = tasks
        .create(&me.id, &title, &input.assignee, input.due, Some(from))
        .await
        .map_err(ApiError::bad_request)?;
    s.execute(
        "INSERT INTO calls_actions (id, call, title, assignee, due, task_path, made_by, at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[store::new_id("a").into(), call.id.as_str().into(), title.into(), input.assignee.as_str().into(), input.due.into(), task.path.into(), me.id.as_str().into(), store::now().into()],
    )
    .await?;
    Ok(ok(&json!({"call": call_json(instance, &call).await?})))
}

macro_rules! with_params {
    ($name:ident, $f:ident) => {
        fn $name(
            request: Request,
            params: PathParams,
            _: ConnectionInfo,
            instance: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $f(&request, &params, &instance).await) })
        }
    };
}

with_params!(h_room, get_room);
with_params!(h_live, live);
with_params!(h_join, join);
with_params!(h_offer, offer);
with_params!(h_state, state);
with_params!(h_leave, leave);
with_params!(h_note, note);
with_params!(h_action, action);

fn object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let route = |method, path, id, summary, handler| Route {
        method,
        path,
        id,
        tag: "calls",
        summary,
        access: Access::Signed,
        response: object,
        handler,
    };
    vec![
        route(
            Method::Get,
            "/calls/live",
            "callsLive",
            "The calls going on in rooms one is in: room, name, kind, startedAt and who is in each.",
            h_live,
        ),
        route(
            Method::Get,
            "/calls/rooms/:room",
            "callsRoom",
            "A room's call going on, and who is in it.",
            h_room,
        ),
        route(
            Method::Post,
            "/calls/rooms/:room/join",
            "callsJoin",
            "Joins the room's call (starting it when there is none): gives one's session.",
            h_join,
        ),
        route(
            Method::Post,
            "/calls/sessions/:session/offer",
            "callsOffer",
            "The browser's WebRTC offer for one's session: {offer}; gives the answer.",
            h_offer,
        ),
        route(
            Method::Post,
            "/calls/sessions/:session/state",
            "callsState",
            "What one shows: {muted, camera, screen, hand, doc}; also says one is still there.",
            h_state,
        ),
        route(
            Method::Post,
            "/calls/sessions/:session/leave",
            "callsLeave",
            "Leaves the call.",
            h_leave,
        ),
        route(
            Method::Post,
            "/calls/sessions/:session/notes",
            "callsNote",
            "A line in the call's live notes: {text}.",
            h_note,
        ),
        route(
            Method::Post,
            "/calls/sessions/:session/actions",
            "callsAction",
            "An action item: a task for someone in the room: {title, assignee, due}.",
            h_action,
        ),
    ]
}
