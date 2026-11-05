//! Bounded worker mailbox: each lane retains only its newest pending task.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Lane {
    Translation,
    Test,
}

struct Pending<T> {
    translation: Option<T>,
    test: Option<T>,
    closed: bool,
}

struct Shared<T> {
    pending: Mutex<Pending<T>>,
    changed: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, Pending<T>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) struct Sender<T> {
    shared: Arc<Shared<T>>,
}

pub(crate) struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        pending: Mutex::new(Pending {
            translation: None,
            test: None,
            closed: false,
        }),
        changed: Condvar::new(),
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared },
    )
}

impl<T> Sender<T> {
    pub(crate) fn send(&self, lane: Lane, value: T) -> Result<(), T> {
        let replaced = {
            let mut pending = self.shared.lock();
            if pending.closed {
                return Err(value);
            }
            match lane {
                Lane::Translation => pending.translation.replace(value),
                Lane::Test => pending.test.replace(value),
            }
        };
        self.shared.changed.notify_one();
        // A task's destructor must not run while holding the mailbox mutex.
        drop(replaced);
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_all();
    }
}

impl<T> Receiver<T> {
    pub(crate) fn recv(&self) -> Option<T> {
        let mut pending = self.shared.lock();
        loop {
            if let Some(value) = pending.test.take().or_else(|| pending.translation.take()) {
                return Some(value);
            }
            if pending.closed {
                return None;
            }
            pending = self
                .shared
                .changed
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let abandoned = {
            let mut pending = self.shared.lock();
            pending.closed = true;
            (pending.translation.take(), pending.test.take())
        };
        self.shared.changed.notify_all();
        drop(abandoned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn busy_worker_receives_only_the_latest_pending_translation() {
        let (sender, receiver) = channel();
        let (busy, started) = mpsc::sync_channel(0);
        let (resume, ready) = mpsc::sync_channel(0);
        let (done, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            assert_eq!(receiver.recv(), Some(1));
            busy.send(()).unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            done.send(receiver.recv()).unwrap();
            done.send(receiver.recv()).unwrap();
        });

        sender.send(Lane::Translation, 1).unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        for value in 2..=100 {
            sender.send(Lane::Translation, value).unwrap();
        }
        drop(sender);
        resume.send(()).unwrap();
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Some(100)
        );
        assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        worker.join().unwrap();
    }

    #[test]
    fn lanes_are_independent_and_test_has_priority() {
        let (sender, receiver) = channel();
        sender.send(Lane::Test, "old test").unwrap();
        sender.send(Lane::Translation, "old translation").unwrap();
        sender.send(Lane::Test, "latest test").unwrap();
        sender
            .send(Lane::Translation, "latest translation")
            .unwrap();
        drop(sender);

        assert_eq!(receiver.recv(), Some("latest test"));
        assert_eq!(receiver.recv(), Some("latest translation"));
        assert_eq!(receiver.recv(), None);
    }

    #[test]
    fn dropping_sender_wakes_waiting_receiver() {
        let (sender, receiver) = channel::<()>();
        let (started, ready) = mpsc::sync_channel(0);
        let (done, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            started.send(()).unwrap();
            done.send(receiver.recv()).unwrap();
        });

        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(sender);
        assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        worker.join().unwrap();
    }

    #[test]
    fn sending_after_receiver_drops_returns_the_unsent_task() {
        let (sender, receiver) = channel();
        drop(receiver);
        assert_eq!(sender.send(Lane::Translation, 42), Err(42));
    }

    #[test]
    fn poisoned_mutex_does_not_disable_delivery_or_shutdown() {
        let (sender, receiver) = channel();
        let shared = Arc::clone(&sender.shared);
        assert!(thread::spawn(move || {
            let _guard = shared.pending.lock().unwrap();
            panic!("simulate an unrelated poisoned lock");
        })
        .join()
        .is_err());

        sender.send(Lane::Translation, 7).unwrap();
        drop(sender);
        assert_eq!(receiver.recv(), Some(7));
        assert_eq!(receiver.recv(), None);
    }
}
