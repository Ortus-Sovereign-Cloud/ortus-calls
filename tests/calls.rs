//! Calls on both shapes, with a media node running on loopback: only a
//! room's people see and join its call; the first to join starts it (the
//! room is told); what one shows (muted, camera, screen, hand) is shared;
//! a browser's offer gets the media node's answer; leaving, or going
//! silent, ends one's part, and the last to go ends the call.

mod common;

use std::sync::Arc;
use std::time::Duration;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use ortus_core::runtime::block_on;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

fn call<A: rust_web_server::application::Application>(
    client: &TestClient<A>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (i16, Value) {
    let url = format!("/api/v1{path}");
    let mut r = if method == "GET" {
        client.get(&url)
    } else {
        client.post(&url)
    };
    r = r
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus");
    if let Some(b) = body {
        r = r
            .header("Content-Type", "application/json")
            .body_bytes(b.to_string().into_bytes());
    }
    let response = r.send();
    (
        response.status(),
        serde_json::from_slice(response.body_bytes()).unwrap_or(Value::Null),
    )
}

/// A browser-like offer (audio and video to send, a data channel), made
/// with str0m on the client side.
fn offer() -> Value {
    use str0m::media::{Direction, MediaKind};
    let mut rtc = str0m::Rtc::builder().build(std::time::Instant::now());
    let mut change = rtc.sdp_api();
    change.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
    change.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    change.add_channel("signalling".into());
    let (offer, _) = change.apply().unwrap();
    serde_json::to_value(&offer).unwrap()
}

#[test]
fn a_rooms_call_from_start_to_end() {
    std::env::set_var("ORTUS_MEDIA_LISTEN", "127.0.0.1:0");
    for (name, instance, _keep) in common::installations() {
        for app in instance.apps.clone() {
            app.provide(&instance, &instance.services);
        }
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
        let olena = signed_in(&instance, "olena@ministry.example", Role::Owner);
        let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
        let anna = signed_in(&instance, "anna@ministry.example", Role::Member);
        let id_of = |e: &str| {
            block_on(ortus_core::auth::accounts::by_email(
                instance.store.as_ref(),
                e,
            ))
            .unwrap()
            .unwrap()
            .id
        };
        let room = format!("flood-{}", name.replace('+', "-"));
        common::put_in_room(&id_of("olena@ministry.example"), &room);
        common::put_in_room(&id_of("marek@ministry.example"), &room);

        // No media node yet: no call.
        let (status, none) = call(
            &client,
            &olena,
            "POST",
            &format!("/calls/rooms/{room}/join"),
            None,
        );
        assert_eq!(409, status, "{name}: {none}");

        // The media node (the media role's task).
        let (stop_tx, stop) = tokio::sync::watch::channel(false);
        let node =
            ortus_core::runtime::io().spawn(ortus_calls::node::run(Arc::clone(&instance), stop));
        for _ in 0..50 {
            if block_on(ortus_calls::store::pick_node(instance.store.as_ref()))
                .unwrap()
                .is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let (status, quiet) = call(
            &client,
            &olena,
            "GET",
            &format!("/calls/rooms/{room}"),
            None,
        );
        assert_eq!(
            (200, Value::Null),
            (status, quiet["call"].clone()),
            "{name}"
        );
        assert_eq!(
            404,
            call(&client, &anna, "GET", &format!("/calls/rooms/{room}"), None).0,
            "{name}: not hers"
        );
        assert_eq!(
            404,
            call(
                &client,
                &anna,
                "POST",
                &format!("/calls/rooms/{room}/join"),
                None
            )
            .0,
            "{name}"
        );

        // Olena starts it; the room hears; Marek joins the same call.
        let (status, hers) = call(
            &client,
            &olena,
            "POST",
            &format!("/calls/rooms/{room}/join"),
            None,
        );
        assert_eq!(200, status, "{name}: {hers}");
        let call_id = hers["call"]["id"].as_str().unwrap().to_string();
        let her_session = hers["session"].as_str().unwrap().to_string();
        let (_, his) = call(
            &client,
            &marek,
            "POST",
            &format!("/calls/rooms/{room}/join"),
            None,
        );
        assert_eq!(call_id, his["call"]["id"].as_str().unwrap(), "{name}");
        let his_session = his["session"].as_str().unwrap().to_string();
        let (_, now) = call(
            &client,
            &anna.clone().replace(&anna, &olena),
            "GET",
            &format!("/calls/rooms/{room}"),
            None,
        );
        assert_eq!(
            2,
            now["call"]["sessions"].as_array().unwrap().len(),
            "{name}"
        );
        assert_eq!(
            2,
            now["call"]["people"].as_object().unwrap().len(),
            "{name}: {now}"
        );

        // What one shows.
        let (_, shown) = call(
            &client,
            &olena,
            "POST",
            &format!("/calls/sessions/{her_session}/state"),
            Some(json!({"muted": true, "hand": true, "screen": "2"})),
        );
        let me = shown["call"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["session"] == her_session.as_str())
            .unwrap()
            .clone();
        assert_eq!(
            (json!(true), json!("2")),
            (me["muted"].clone(), me["screen"].clone()),
            "{name}"
        );
        assert!(me["hand"].as_i64().is_some(), "{name}");
        let (_, cleared) = call(
            &client,
            &olena,
            "POST",
            &format!("/calls/sessions/{her_session}/state"),
            Some(json!({"screen": null, "hand": false})),
        );
        let me = cleared["call"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["session"] == her_session.as_str())
            .unwrap()
            .clone();
        assert_eq!(
            (Value::Null, Value::Null),
            (me["screen"].clone(), me["hand"].clone()),
            "{name}"
        );
        // One's own session only.
        assert_eq!(
            404,
            call(
                &client,
                &marek,
                "POST",
                &format!("/calls/sessions/{her_session}/state"),
                Some(json!({"muted": false}))
            )
            .0,
            "{name}"
        );

        // A browser's offer: the media node answers, with where to send.
        let (status, answered) = call(
            &client,
            &olena,
            "POST",
            &format!("/calls/sessions/{her_session}/offer"),
            Some(json!({"offer": offer()})),
        );
        assert_eq!(200, status, "{name}: {answered}");
        let sdp = answered["answer"]["sdp"].as_str().unwrap_or_default();
        assert!(
            sdp.contains("a=candidate") && sdp.contains("127.0.0.1"),
            "{name}: {sdp}"
        );
        assert_eq!(
            400,
            call(
                &client,
                &olena,
                "POST",
                &format!("/calls/sessions/{her_session}/offer"),
                Some(json!({"offer": {"type": "offer", "sdp": "nonsense"}}))
            )
            .0,
            "{name}"
        );

        // Marek leaves; Olena goes silent; the sweep ends the call.
        assert_eq!(
            200,
            call(
                &client,
                &marek,
                "POST",
                &format!("/calls/sessions/{his_session}/leave"),
                None
            )
            .0,
            "{name}"
        );
        block_on(instance.store.execute(
            "UPDATE calls_sessions SET seen_at = 0 WHERE id = $1",
            &[her_session.as_str().into()],
        ))
        .unwrap();
        block_on(ortus_calls::sweep(&instance)).unwrap();
        let (_, over) = call(
            &client,
            &olena,
            "GET",
            &format!("/calls/rooms/{room}"),
            None,
        );
        assert_eq!(Value::Null, over["call"], "{name}: {over}");
        let ended = block_on(instance.store.query(
            "SELECT ended_at FROM calls_calls WHERE id = $1",
            &[call_id.as_str().into()],
        ))
        .unwrap();
        assert!(ended[0].int(0).is_some(), "{name}");

        stop_tx.send(true).unwrap();
        let _ = block_on(node);
    }
}
