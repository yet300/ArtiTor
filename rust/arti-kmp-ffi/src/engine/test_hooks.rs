//! Deterministic test-only scheduling and publication probes.
use super::*;

#[cfg(test)]
#[derive(Default)]
pub(super) struct TestSignal {
    value: Mutex<usize>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
impl TestSignal {
    pub(super) fn signal(&self) {
        *self.value.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    pub(super) fn wait(&self) {
        self.wait_for(1);
    }

    pub(super) fn wait_for(&self, count: usize) {
        let mut value = self.value.lock().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while *value < count {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let (next, timeout) = self.changed.wait_timeout(value, remaining).unwrap();
            value = next;
            assert!(
                !timeout.timed_out() || *value >= count,
                "deterministic test gate timed out"
            );
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct TestCommitGate {
    pub(super) arrived: TestSignal,
    pub(super) release: TestSignal,
    pub(super) bound_port: Mutex<Option<u16>>,
}

#[cfg(test)]
impl TestCommitGate {
    pub(super) fn arrive_and_wait(&self, port: Option<u16>) {
        *self.bound_port.lock().unwrap() = port;
        self.arrived.signal();
        self.release.wait();
    }
}

/// Observes the actual publication barrier receiver, rather than inferring
/// listener execution from TCP backlog acceptance or elapsed time.
#[cfg(test)]
#[derive(Default)]
pub(super) struct TestListenerProbe {
    pub(super) barrier: Mutex<Option<Arc<Mutex<oneshot::Receiver<()>>>>>,
    pub(super) polled: TestSignal,
    pub(super) dispatch_checked: TestSignal,
    pub(super) dispatch_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl TestListenerProbe {
    pub(super) fn assert_barrier_pending(&self) {
        self.polled.wait();
        let barrier = self.barrier.lock().unwrap().as_ref().unwrap().clone();
        assert!(
            matches!(
                barrier.lock().unwrap().try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "listener barrier released before ACTIVE callback completed"
        );
        assert_eq!(self.dispatch_count.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
pub(super) async fn await_test_session_barrier(rx: oneshot::Receiver<()>, shared: &Shared) {
    let probe = shared.test_listener_probe.lock().unwrap().clone();
    if let Some(probe) = probe {
        let rx = Arc::new(Mutex::new(rx));
        *probe.barrier.lock().unwrap() = Some(rx.clone());
        let _ = std::future::poll_fn(|cx| {
            let poll = std::future::Future::poll(std::pin::Pin::new(&mut *rx.lock().unwrap()), cx);
            probe.polled.signal();
            poll
        })
        .await;
    } else {
        let _ = rx.await;
    }
}
