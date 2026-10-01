//! Inject only into a disposable copy as engine::tests::offline_pause_reproducer.
//! This diagnostic PASSES when the current teardown defect is reproduced.
//! No external target, bootstrap, CONNECT request, or production edit is used.
use super::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{mpsc, Condvar};

struct HeldRootCallback {
    published: mpsc::Sender<u16>,
    release: (StdMutex<bool>, Condvar),
}
impl HeldRootCallback {
    fn release(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}
impl StatusListener for HeldRootCallback {
    fn on_status(&self, state: TorState, _: u32, port: Option<u16>, _: String) {
        if state == TorState::Running {
            self.published.send(port.unwrap()).unwrap();
            let released = self.release.0.lock().unwrap();
            let (released, bound) = self.release.1.wait_timeout_while(
                released, Duration::from_secs(10), |done| !*done,
            ).unwrap();
            assert!(*released && !bound.timed_out(), "diagnostic callback bound");
        }
    }
    fn on_log(&self, _: String) {}
    fn on_error(&self, _: ArtiErrorDetail) {}
}
struct ReleaseOnDrop(Arc<HeldRootCallback>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) { self.0.release(); }
}

#[test]
fn deterministically_reproduces_pause_tcp_teardown_gap() {
    for iteration in 0..10 {
        let (engine, recorder) = running_engine(&format!("offline-pause-gap-{iteration}"));
        let session = engine.create_session(Box::new(recorder)).unwrap();
        let (tx, rx) = mpsc::channel();
        let callback = Arc::new(HeldRootCallback {
            published: tx,
            release: (StdMutex::new(false), Condvar::new()),
        });
        let _release = ReleaseOnDrop(callback.clone());
        let shared = {
            let mut inner = engine.inner.lock().unwrap();
            let shared = inner.shared.clone();
            shared.set_listener(callback.clone());
            let client = shared.client().unwrap();
            let revision = shared.worker_revision.load(Ordering::SeqCst);
            let task_shared = shared.clone();
            inner.worker = Some(inner.runtime.as_ref().unwrap().spawn(async move {
                root_socks::run_socks_worker(client, 0, task_shared, None, revision).await.unwrap();
            }));
            shared
        };
        // Root is bound and has published RUNNING. Its synchronous callback is
        // held to force a scheduling interval before the worker can be dropped.
        let port = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let before_revision = shared.worker_revision.load(Ordering::SeqCst);
        engine.pause();
        assert_eq!(shared.engine_state(), TorState::Paused);
        assert_eq!(shared.bound_port.load(Ordering::SeqCst), 0);
        assert!(shared.client().is_some());
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        let after_revision = shared.worker_revision.load(Ordering::SeqCst);
        let endpoint = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&endpoint, Duration::from_secs(1))
            .expect("current defect: PAUSED old root still accepts TCP");
        stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        stream.write_all(&[5, 1, 0]).unwrap();
        let mut reply = [0; 2];
        let read = stream.read(&mut reply).unwrap_err();
        assert!(matches!(read.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
        println!("DEFECT_REPRODUCED,iteration={iteration},state=PAUSED,bound_port=0,old_port={port},tcp_connected=true,socks_greeting=timeout,worker_revision={before_revision}->{after_revision},session=PAUSED/null");
        drop(stream);
        callback.release();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)) {
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => break,
                other => {
                    assert!(std::time::Instant::now() < deadline, "listener did not close after callback release: {other:?}");
                    std::thread::yield_now();
                }
            }
        }
        println!("AFTER_CALLBACK_RELEASE,iteration={iteration},old_port_refused=true");
        engine.shutdown();
    }
}
