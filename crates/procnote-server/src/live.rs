use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

use procnote_core::event::types::ExecutionId;

/// Fan-out of "execution X now has N events" signals to connected clients.
///
/// It carries only the event count (`seq`); clients fetch the state
/// themselves, so nothing about an execution is held in memory here. Only
/// writes made through this server are announced.
#[derive(Default)]
pub struct LiveUpdates {
    channels: Mutex<HashMap<ExecutionId, broadcast::Sender<u64>>>,
}

impl LiveUpdates {
    /// Receive the `seq` of every later write to `execution_id`.
    pub fn subscribe(&self, execution_id: ExecutionId) -> broadcast::Receiver<u64> {
        let mut channels = self.lock();
        channels.retain(|_, sender| sender.receiver_count() > 0);
        channels
            .entry(execution_id)
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }

    /// Announce that `execution_id` now has `seq` events.
    pub fn publish(&self, execution_id: ExecutionId, seq: u64) {
        if let Some(sender) = self.lock().get(&execution_id) {
            // An error only means nobody is listening.
            let _ = sender.send(seq);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ExecutionId, broadcast::Sender<u64>>> {
        self.channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_only_their_execution() {
        let live = LiveUpdates::default();
        let (a, b) = (ExecutionId::new_v4(), ExecutionId::new_v4());
        let mut on_a = live.subscribe(a);
        let mut on_b = live.subscribe(b);

        live.publish(a, 7);

        assert_eq!(on_a.recv().await.unwrap(), 7);
        assert!(on_b.try_recv().is_err());
    }

    #[test]
    fn publishing_without_subscribers_is_harmless() {
        let live = LiveUpdates::default();

        live.publish(ExecutionId::new_v4(), 1);
    }

    #[test]
    fn channels_without_subscribers_are_dropped() {
        let live = LiveUpdates::default();
        let id = ExecutionId::new_v4();
        drop(live.subscribe(id));

        let _other = live.subscribe(ExecutionId::new_v4());

        assert!(!live.lock().contains_key(&id));
    }
}
