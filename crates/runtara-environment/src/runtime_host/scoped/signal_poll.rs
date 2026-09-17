//! One bounded lifecycle poll stream shared by a root and all its descendants.
use std::{
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};

/// Keep only control identity, never the command payload, in the shared cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Receipt {
    pub command_id: String,
    pub kind: i32,
}

type PollResult = Result<Option<Receipt>, String>;

pub(super) struct SignalPoll {
    interval: Duration,
    revision: AtomicU64,
    cached: Mutex<Option<(u64, Instant, PollResult)>>,
}
impl SignalPoll {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            revision: AtomicU64::new(0),
            cached: Mutex::new(None),
        }
    }

    /// Positive, negative and failed reads are shared. Returning a cached
    /// positive to every caller is essential: throttling must not hide a stop
    /// from siblings after the first child observes it.
    ///
    /// Explicit checkpoint receipts request a fresh read to preserve exact
    /// replacement-command checks. Like checkpoint IO itself, these are outside
    /// the tight-loop polling budget. Only one read can be in flight per owner.
    pub async fn poll<F, Fut>(&self, fresh: bool, read: F) -> PollResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = PollResult>,
    {
        let mut cached = self.cached.lock().await;
        let revision = self.revision.load(Ordering::Acquire);
        if !fresh
            && let Some((cached_revision, at, result)) = &*cached
            && *cached_revision == revision
            && at.elapsed() < self.interval
        {
            return result.clone();
        }
        // Retain no spawned worker: dropping a cancelled caller drops its read
        // and lock, allowing a sibling to retry. Cache only completed reads.
        let result = read().await;
        // Invalidation never waits for this read. Retain its starting revision
        // so a read completed after invalidation cannot poison the next poll.
        *cached = Some((revision, Instant::now(), result.clone()));
        result
    }

    /// Trusted checkpoint/sleep IO saw a pending command. Do not let an older
    /// cached empty result hide it from the guest's next signal check.
    pub fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests;
