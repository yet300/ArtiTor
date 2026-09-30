//! Framing, selected-client dispatch and current/stale listener failure regressions.
use super::*;

#[test]
fn socks_request_parses_ipv4_target() {
    let buf = [0x05, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0x1F, 0x90];
    assert_eq!(parse_socks_request(&buf, 10), Ok(("1.2.3.4".into(), 8080)));
}

#[test]
fn socks_request_parses_hostname_target() {
    let host = b"example.com";
    let mut buf = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
    buf.extend_from_slice(host);
    buf.extend_from_slice(&[0x00, 0x50]);
    let n = buf.len();
    assert_eq!(parse_socks_request(&buf, n), Ok(("example.com".into(), 80)));
}

#[test]
fn socks_request_parses_ipv6_target() {
    // ::1 port 443.
    let mut buf = vec![0x05, 0x01, 0x00, 0x04];
    buf.extend_from_slice(&[0u8; 15]);
    buf.push(1);
    buf.extend_from_slice(&[0x01, 0xBB]);
    assert_eq!(buf.len(), 22);
    assert_eq!(
        parse_socks_request(&buf, 22),
        Ok(("0:0:0:0:0:0:0:1".into(), 443))
    );
}

#[test]
fn socks_request_rejects_non_connect_truncated_and_unknown() {
    // BIND command.
    let bind = [0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80];
    assert_eq!(
        parse_socks_request(&bind, 10),
        Err(SocksRequestError::UnsupportedCommand)
    );
    // Unknown address type.
    let atyp = [0x05, 0x01, 0x00, 0x05, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        parse_socks_request(&atyp, 10),
        Err(SocksRequestError::UnsupportedAtyp)
    );
    // Truncated IPv6.
    let short6 = [0x05, 0x01, 0x00, 0x04, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        parse_socks_request(&short6, 10),
        Err(SocksRequestError::TruncatedIpv6)
    );
    // Short request / bad version / domain overrun: malformed.
    assert_eq!(
        parse_socks_request(&[0x05, 0x01], 2),
        Err(SocksRequestError::Malformed)
    );
    let badver = [0x04, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0, 80];
    assert_eq!(
        parse_socks_request(&badver, 10),
        Err(SocksRequestError::Malformed)
    );
    let mut over = vec![0x05, 0x01, 0x00, 0x03, 20, b'a', b'b'];
    over.extend_from_slice(&[0x00, 0x50]);
    let n = over.len();
    assert_eq!(
        parse_socks_request(&over, n),
        Err(SocksRequestError::Malformed)
    );
}

#[tokio::test]
async fn socks_rejects_non_connect_command_without_tor() {
    // Wire-level rejection happens BEFORE any Tor use: no bootstrap, no
    // network. BIND must draw 0x07.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.expect("accept");
        handle_socks(sock, test_client("socks-reject")).await
    });
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    #[cfg(test)]
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut greeting = [0u8; 2];
    client.read_exact(&mut greeting).await.unwrap();
    assert_eq!(greeting, [0x05, 0x00]);
    client
        .write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80])
        .await
        .unwrap();
    let mut reply = [0u8; 10];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x07, "BIND must be refused with 0x07");
    let res = server.await.expect("server task");
    assert!(res.is_err(), "handler reports the rejection");
}

#[test]
fn fatal_accept_error_demotes_only_current_listener() {
    struct InjectListener {
        shared: Weak<Shared>,
        paused: Arc<TestSignal>,
    }
    impl SessionStatusListener for InjectListener {
        fn on_session_status(&self, id: String, state: SessionState, _: Option<u16>, _: u64) {
            let shared = self.shared.upgrade().unwrap();
            assert!(
                shared.transition_gate.try_lock().is_ok(),
                "callback outside transaction"
            );
            if state == SessionState::Active {
                *shared.test_accept_failure.lock().unwrap() = Some(id);
            }
            if state == SessionState::Paused {
                self.paused.signal();
            }
        }
    }
    let (engine, _) = running_engine("fatal-accept");
    let shared = engine.inner.lock().unwrap().shared.clone();
    let paused = Arc::new(TestSignal::default());
    let session = engine
        .create_session(Box::new(InjectListener {
            shared: Arc::downgrade(&shared),
            paused: paused.clone(),
        }))
        .unwrap();
    paused.wait();
    assert_eq!(session.status_snapshot().state, SessionState::Paused);
    assert_eq!(session.status_snapshot().port, None);
    assert_eq!(session.status_snapshot().revision, 2);
    assert_eq!(shared.engine_state(), TorState::Running);
    engine.shutdown();
}

#[test]
fn stale_listener_failure_cannot_demote_rebound_closed_or_invalidated_session() {
    let (engine, rec) = running_engine("stale-listener-failure");
    let session = engine.create_session(Box::new(rec.clone())).unwrap();
    let shared = engine.inner.lock().unwrap().shared.clone();
    let old_revision = session.status_snapshot().revision;
    engine.pause();
    *shared.engine_state.lock().unwrap() = TorState::Running;
    let handle = engine
        .inner
        .lock()
        .unwrap()
        .runtime
        .as_ref()
        .unwrap()
        .handle()
        .clone();
    rebind_paused_sessions(&shared, &handle);
    let current = session.status_snapshot();
    assert_eq!(current.state, SessionState::Active);
    session_listener_failed(
        &Arc::downgrade(&shared),
        &session.id(),
        session.generation(),
        old_revision,
    );
    assert_eq!(session.status_snapshot(), current);
    session.close_session();
    session_listener_failed(
        &Arc::downgrade(&shared),
        &session.id(),
        session.generation(),
        current.revision,
    );
    assert_eq!(session.status_snapshot().state, SessionState::Closed);
    let other = engine.create_session(Box::new(rec)).unwrap();
    let rev = other.status_snapshot().revision;
    engine.shutdown();
    session_listener_failed(
        &Arc::downgrade(&shared),
        &other.id(),
        other.generation(),
        rev,
    );
    assert_eq!(other.status_snapshot().state, SessionState::Invalidated);
}

#[test]
fn actual_root_and_two_session_dispatches_select_distinct_client_handles() {
    let (engine, rec) = running_engine("dispatch-identities");
    let shared = engine.inner.lock().unwrap().shared.clone();
    let handle = engine
        .inner
        .lock()
        .unwrap()
        .runtime
        .as_ref()
        .unwrap()
        .handle()
        .clone();
    let root = shared.client().unwrap();
    let root_ptr = Arc::as_ptr(&root) as usize;
    let (ready_tx, ready_rx) = oneshot::channel();
    let root_shared = shared.clone();
    let worker = handle.spawn(async move {
        run_socks(root, 0, root_shared, Some(ready_tx))
            .await
            .unwrap();
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(ready_rx).unwrap();
    let a = engine.create_session(Box::new(rec.clone())).unwrap();
    let b = engine.create_session(Box::new(rec)).unwrap();
    let (a_ptr, b_ptr) = {
        let entries = shared.sessions.lock().unwrap();
        (
            Arc::as_ptr(&entries[&a.id()].isolated) as usize,
            Arc::as_ptr(&entries[&b.id()].isolated) as usize,
        )
    };
    assert_ne!(root_ptr, a_ptr);
    assert_ne!(root_ptr, b_ptr);
    assert_ne!(a_ptr, b_ptr);
    for port in [
        engine.socks_port().unwrap(),
        a.status_snapshot().port.unwrap(),
        b.status_snapshot().port.unwrap(),
    ] {
        rt.block_on(async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream.write_all(&[5, 1, 0]).await.unwrap();
            let mut reply = [0; 2];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reply, [5, 0]);
        });
    }
    assert_eq!(
        *shared.test_dispatch_clients.lock().unwrap(),
        vec![
            (None, root_ptr),
            (Some(a.id()), a_ptr),
            (Some(b.id()), b_ptr)
        ]
    );
    worker.abort();
    engine.shutdown();
}
