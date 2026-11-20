//! Fresh installations for the calls tests: SQLite with a local directory,
//! and PostgreSQL with S3 (ORTUS_TEST_POSTGRES, ORTUS_TEST_S3_*), so every
//! storage test runs on both databases and both file stores.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ortus_core::config::Config;
use ortus_core::files::{Files, S3Settings};
use ortus_core::instance::{schemas, Instance};
use ortus_core::runtime::block_on;
use ortus_core::secrets::Keyring;
use ortus_core::storage::{AsyncStore, PostgresStore, SqliteStore};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn unique() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst),
        ortus_core::program::now_ms()
    )
}

/// A directory removed when dropped.
pub struct Dir(pub std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn dir() -> Dir {
    let path = std::env::temp_dir().join(format!("ortus-calls-test-{}", unique()));
    std::fs::create_dir_all(&path).unwrap();
    Dir(path)
}

fn assemble(store: Arc<dyn AsyncStore>, files: Files) -> Arc<Instance> {
    let apps = ortus_calls::apps();
    block_on(ortus_core::storage::install(
        store.as_ref(),
        &schemas(&apps),
        0,
    ))
    .unwrap();
    let config = Config::load(None, &|_| None).unwrap();
    let instance = Instance::assemble(config, store, Arc::new(files), apps, Keyring::for_tests());
    let chat: Arc<dyn ortus_core::interfaces::Chat> = Arc::new(FakeChat::default());
    instance.services.provide("chat", chat);
    instance
}

/// Chat's rooms for the tests: "flood-response" with olena and marek in
/// it, "secret" with olena alone; notices are kept to look at.
#[derive(Default)]
pub struct FakeChat {
    pub notices: std::sync::Mutex<Vec<(String, String, String)>>,
}

pub static MEMBERS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Puts a person (by user id) in a room of the fake chat.
pub fn put_in_room(user: &str, room: &str) {
    MEMBERS
        .lock()
        .unwrap()
        .push((user.to_string(), room.to_string()));
}

impl ortus_core::interfaces::Chat for FakeChat {
    fn post<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<ortus_core::interfaces::ItemRef, String>> {
        Box::pin(async { Err("not here".into()) })
    }

    fn room<'a>(
        &'a self,
        user: &'a str,
        room: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Option<ortus_core::interfaces::ChatRoom>, String>>
    {
        Box::pin(async move {
            let members: Vec<String> = MEMBERS
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, r)| r == room)
                .map(|(u, _)| u.clone())
                .collect();
            if !members.iter().any(|u| u == user) {
                return Ok(None);
            }
            Ok(Some(ortus_core::interfaces::ChatRoom {
                id: room.into(),
                name: room.into(),
                kind: "channel".into(),
                encrypted: false,
                members,
            }))
        })
    }

    fn notice<'a>(
        &'a self,
        user: &'a str,
        room: &'a str,
        kind: &'a str,
        about: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<(), String>> {
        self.notices
            .lock()
            .unwrap()
            .push((room.to_string(), kind.to_string(), about.to_string()));
        let _ = user;
        Box::pin(async { Ok(()) })
    }
}

/// Every installation shape the environment allows, by name.
pub fn installations() -> Vec<(&'static str, Arc<Instance>, Option<Dir>)> {
    let mut all = Vec::new();
    let dir = dir();
    let store: Arc<dyn AsyncStore> = Arc::new(SqliteStore::open(&dir.0.join("ortus.db")).unwrap());
    let files = Files::local(&dir.0.join("files")).unwrap();
    all.push(("sqlite+local", assemble(store, files), Some(dir)));

    match std::env::var("ORTUS_TEST_POSTGRES") {
        Ok(url) => {
            let schema = format!("calls_{}", unique().replace('-', "_"));
            block_on(
                PostgresStore::connect(&url, 1)
                    .unwrap()
                    .execute(&format!("CREATE SCHEMA {schema}"), &[]),
            )
            .unwrap();
            let separator = if url.contains('?') { '&' } else { '?' };
            let store: Arc<dyn AsyncStore> = Arc::new(
                PostgresStore::connect(
                    &format!("{url}{separator}options=-c%20search_path%3D{schema}"),
                    4,
                )
                .unwrap(),
            );
            let var =
                |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
            let files = match std::env::var("ORTUS_TEST_S3_ENDPOINT") {
                Ok(endpoint) => Files::s3(&S3Settings {
                    endpoint,
                    bucket: var("ORTUS_TEST_S3_BUCKET"),
                    region: std::env::var("ORTUS_TEST_S3_REGION")
                        .unwrap_or_else(|_| "us-east-1".into()),
                    access_key: var("ORTUS_TEST_S3_ACCESS_KEY"),
                    secret_key: var("ORTUS_TEST_S3_SECRET_KEY"),
                    root: format!("ortus-calls-{}", unique()),
                })
                .unwrap(),
                Err(_) => {
                    assert!(
                        std::env::var("ORTUS_REQUIRE_S3").is_err(),
                        "ORTUS_REQUIRE_S3 is set but ORTUS_TEST_S3_ENDPOINT is not"
                    );
                    Files::local(&dir_leak()).unwrap()
                }
            };
            all.push(("postgres+s3", assemble(store, files), None));
        }
        Err(_) => assert!(
            std::env::var("ORTUS_REQUIRE_POSTGRES").is_err(),
            "ORTUS_REQUIRE_POSTGRES is set but ORTUS_TEST_POSTGRES is not"
        ),
    }
    all
}

fn dir_leak() -> std::path::PathBuf {
    let d = dir();
    let path = d.0.clone();
    std::mem::forget(d);
    path
}
