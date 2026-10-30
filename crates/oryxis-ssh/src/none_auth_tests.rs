//! Hermetic tests for the `none` request at the head of `AuthMethod::Auto`
//! (RFC 4252 §5.2, issue #279). An in-process russh server over loopback
//! TCP counts every auth request it receives, so a test proves the
//! ORDER the engine sent them in, not only that the connect worked.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::sftp_harness::HARNESS_HOST_KEY;
use crate::{HostKeyCheckCallback, HostKeyStatus, SshEngine};
use oryxis_core::models::connection::{AuthMethod, Connection};
use russh::server::{Auth, PublicKeyOffer, Response};

const TEST_USER: &str = "root";
const TEST_PASS: &str = "correct-horse";

/// The auth requests one server saw, by method.
#[derive(Default)]
struct Seen {
    none: AtomicUsize,
    password: AtomicUsize,
    publickey: AtomicUsize,
    keyboard_interactive: AtomicUsize,
}

impl Seen {
    fn others(&self) -> usize {
        self.password.load(Ordering::SeqCst)
            + self.publickey.load(Ordering::SeqCst)
            + self.keyboard_interactive.load(Ordering::SeqCst)
    }
}

/// `accept_none` is the dropbear left open: `none` alone lets the user
/// in. Otherwise `none` is refused and only `TEST_PASS` is taken. Every
/// public key is refused at the OFFER, so the agent sweep between the
/// two never asks the test process's own agent for a signature (a
/// confirm-gated key there would park the test on a person).
struct CountingHandler {
    accept_none: bool,
    seen: Arc<Seen>,
}

impl russh::server::Handler for CountingHandler {
    type Error = russh::Error;

    async fn auth_none(&mut self, user: &str) -> Result<Auth, Self::Error> {
        self.seen.none.fetch_add(1, Ordering::SeqCst);
        if self.accept_none && user == TEST_USER {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.seen.password.fetch_add(1, Ordering::SeqCst);
        if user == TEST_USER && password == TEST_PASS {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _offer: PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        self.seen.publickey.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::reject())
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        _response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        self.seen.keyboard_interactive.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::reject())
    }
}

/// Spawn the loopback server; returns its port and the request tally.
async fn spawn_server(accept_none: bool) -> (u16, Arc<Seen>) {
    use russh::keys::PrivateKey;

    let mut config = russh::server::Config::default();
    config
        .keys
        .push(PrivateKey::from_openssh(HARNESS_HOST_KEY).expect("parse host key"));
    config.auth_rejection_time = Duration::ZERO;
    config.auth_rejection_time_initial = Some(Duration::ZERO);
    let config = Arc::new(config);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();
    let seen = Arc::new(Seen::default());

    let server_seen = seen.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let config = config.clone();
            let handler = CountingHandler { accept_none, seen: server_seen.clone() };
            tokio::spawn(async move {
                if let Ok(running) = russh::server::run_stream(config, socket, handler).await {
                    let _ = running.await;
                }
            });
        }
    });

    (port, seen)
}

fn loopback_conn(port: u16, auth_method: AuthMethod) -> Connection {
    let mut conn = Connection::new("none-auth-test", "127.0.0.1");
    conn.port = port;
    conn.username = Some(TEST_USER.to_string());
    conn.auth_method = auth_method;
    conn
}

fn engine() -> SshEngine {
    let accept_all: HostKeyCheckCallback = Arc::new(|_, _, _, _| HostKeyStatus::Known);
    SshEngine::new()
        .with_host_key_check(accept_all)
        .with_connect_timeout(Duration::from_secs(10))
        .with_auth_timeout(Duration::from_secs(10))
}

/// Issue #279: a host with no credential at all, on a server that takes
/// `none`, authenticates, and `none` is the FIRST and only request.
#[tokio::test]
async fn auto_logs_in_where_the_server_takes_none() {
    let (port, seen) = spawn_server(true).await;
    let conn = loopback_conn(port, AuthMethod::Auto);
    let engine = engine();

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, None, None)
        .await
        .expect("a server granting `none` must let Auto in with no credential");
    assert_eq!(seen.none.load(Ordering::SeqCst), 1);
    assert_eq!(seen.others(), 0, "nothing may be offered before `none` answers");
}

/// The common server refuses `none`: the refusal is a probe, not a
/// failure, and the stored password still logs in after it.
#[tokio::test]
async fn auto_goes_on_after_none_is_refused() {
    let (port, seen) = spawn_server(false).await;
    let conn = loopback_conn(port, AuthMethod::Auto);
    let engine = engine();

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, Some(TEST_PASS), None)
        .await
        .expect("a refused `none` must leave the password step to run");
    assert_eq!(seen.none.load(Ordering::SeqCst), 1);
    assert_eq!(seen.password.load(Ordering::SeqCst), 1);
}

/// No credential and a server that refuses `none`: the error names the
/// probe among the methods tried.
#[tokio::test]
async fn auto_reports_none_among_the_methods_tried() {
    let (port, _seen) = spawn_server(false).await;
    let conn = loopback_conn(port, AuthMethod::Auto);
    let engine = engine();

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    let err = engine
        .do_authenticate(&mut handle, &conn, None, None)
        .await
        .expect_err("nothing the server takes was available");
    let msg = err.to_string();
    assert!(msg.contains("Tried: none, agent"), "unexpected error: {msg}");
}

/// An explicit method sends exactly its credential: a server that would
/// take `none` never sees one from a Password host.
#[tokio::test]
async fn an_explicit_method_never_sends_none() {
    let (port, seen) = spawn_server(true).await;
    let conn = loopback_conn(port, AuthMethod::Password);
    let engine = engine();

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, Some(TEST_PASS), None)
        .await
        .expect("the password is accepted");
    assert_eq!(seen.none.load(Ordering::SeqCst), 0);
    assert_eq!(seen.password.load(Ordering::SeqCst), 1);
}
