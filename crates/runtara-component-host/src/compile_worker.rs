// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A tenant's warm compiler (SYN-930): one compiler, kept running elsewhere
//! as the tenant, takes the tenant's compiles one after another instead of a
//! fresh compiler starting for each.
//!
//! The host queues a compile ([`CompileQueues::submit`]) and awaits its
//! result. The worker's launch ([`crate::api_launches::ApiLaunches::begin_compile_worker`])
//! asks for the next of its tenant's compiles (`GET /v1/compiles/next`),
//! fetches the components it reads by SHA-256
//! (`GET /v1/compiles/{id}/artifacts/{sha}`, in slices), hands back the
//! composed component (`PUT /v1/compiles/{id}/artifacts/{sha}`, in parts) and
//! posts what it made, or why it failed (`POST /v1/compiles/{id}/outcome`).
//! A worker is served only its own tenant's compiles, and only the one it took.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::Digest;

/// What a queued compile ended with: the outcome its worker posted, and the
/// composed component it handed back, if whole.
#[derive(Debug)]
pub struct CompileResult {
    pub outcome: Vec<u8>,
    pub made: Option<Vec<u8>>,
}

/// The most a compile hands back, whatever its parts say.
const MADE_LIMIT: usize = 64 * 1024 * 1024;
/// The most one slice of a component carries: what a node's host origin
/// answers at most.
const MAX_SLICE: u64 = 8 * 1024 * 1024;
/// The longest a worker's ask for the next compile waits.
pub const MAX_WAIT: Duration = Duration::from_secs(30);

struct Job {
    tenant: String,
    request: Vec<u8>,
    components: BTreeMap<String, PathBuf>,
    /// The worker launch that took it.
    worker: Mutex<Option<String>>,
    made: Mutex<Made>,
    done: Mutex<Option<tokio::sync::oneshot::Sender<CompileResult>>>,
}

/// The composed component being handed back: its parts so far, of the
/// component whose SHA-256 and part count the first part named.
#[derive(Default)]
struct Made {
    expected: Option<(String, u32)>,
    parts: BTreeMap<u32, Vec<u8>>,
    whole: Option<Vec<u8>>,
}

/// Each tenant's queued compiles, and every compile not yet ended, by id.
#[derive(Default)]
pub struct CompileQueues {
    state: Mutex<Queues>,
    queued: tokio::sync::Notify,
}

#[derive(Default)]
struct Queues {
    waiting: HashMap<String, VecDeque<String>>,
    jobs: HashMap<String, Arc<Job>>,
}

/// Why a worker's call was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a compile this worker took.
    NotYours,
    /// Not a component of this compile.
    NotAComponent,
    /// The parts do not make the component they name, or name another.
    BadPart(&'static str),
    /// Reading a component failed.
    Unavailable(String),
}

impl CompileQueues {
    /// Queues `tenant`'s compile of `request`, which may fetch `components`
    /// (each file by its SHA-256): answers its id and its result, which ends
    /// when a worker posts its outcome, or with an error when it is cancelled.
    pub fn submit(
        &self,
        tenant: &str,
        request: Vec<u8>,
        components: BTreeMap<String, PathBuf>,
    ) -> (String, tokio::sync::oneshot::Receiver<CompileResult>) {
        // Unique in this process; a worker is served a compile only if it took it.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = format!(
            "c{:x}{:x}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let (done, result) = tokio::sync::oneshot::channel();
        let job = Arc::new(Job {
            tenant: tenant.to_owned(),
            request,
            components,
            worker: Mutex::default(),
            made: Mutex::default(),
            done: Mutex::new(Some(done)),
        });
        {
            let mut state = self.state.lock().expect("compile queues poisoned");
            state.jobs.insert(id.clone(), job);
            state
                .waiting
                .entry(tenant.to_owned())
                .or_default()
                .push_back(id.clone());
        }
        self.queued.notify_waiters();
        (id, result)
    }

    /// Forgets the compile `id`, whether queued, taken or ended: its result
    /// ends with an error, and its worker is served it no more.
    pub fn cancel(&self, id: &str) {
        let mut state = self.state.lock().expect("compile queues poisoned");
        if let Some(job) = state.jobs.remove(id)
            && let Some(waiting) = state.waiting.get_mut(&job.tenant)
        {
            waiting.retain(|queued| queued != id);
        }
    }

    /// The next of `tenant`'s queued compiles, taken by the worker launch
    /// `worker`, waiting up to `wait` for one: its id and request.
    pub async fn next(
        &self,
        tenant: &str,
        worker: &str,
        wait: Duration,
    ) -> Option<(String, Vec<u8>)> {
        let deadline = tokio::time::Instant::now() + wait.min(MAX_WAIT);
        loop {
            // Registered before looking, so a compile queued meanwhile wakes it.
            let queued = self.queued.notified();
            tokio::pin!(queued);
            queued.as_mut().enable();
            if let Some(taken) = self.take(tenant, worker) {
                return Some(taken);
            }
            if tokio::time::timeout_at(deadline, queued).await.is_err() {
                return None;
            }
        }
    }

    fn take(&self, tenant: &str, worker: &str) -> Option<(String, Vec<u8>)> {
        let mut state = self.state.lock().expect("compile queues poisoned");
        let Queues { waiting, jobs } = &mut *state;
        let waiting = waiting.get_mut(tenant)?;
        while let Some(id) = waiting.pop_front() {
            if let Some(job) = jobs.get(&id) {
                *job.worker.lock().expect("compile job poisoned") = Some(worker.to_owned());
                return Some((id, job.request.clone()));
            }
        }
        None
    }

    /// The compile `id`, if `tenant`'s worker `worker` took it.
    fn taken(&self, id: &str, tenant: &str, worker: &str) -> Result<Arc<Job>, Refusal> {
        let job = self
            .state
            .lock()
            .expect("compile queues poisoned")
            .jobs
            .get(id)
            .cloned()
            .ok_or(Refusal::NotYours)?;
        let ours = job.tenant == tenant
            && job.worker.lock().expect("compile job poisoned").as_deref() == Some(worker);
        if ours {
            Ok(job)
        } else {
            Err(Refusal::NotYours)
        }
    }

    /// The bytes `offset`..`offset + length` of the component `sha256` of the
    /// compile `id`, and its whole size.
    pub async fn artifact(
        &self,
        id: &str,
        tenant: &str,
        worker: &str,
        sha256: &str,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(Vec<u8>, u64), Refusal> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let job = self.taken(id, tenant, worker)?;
        let path = job
            .components
            .get(sha256)
            .cloned()
            .ok_or(Refusal::NotAComponent)?;
        let unavailable = |error: std::io::Error| Refusal::Unavailable(error.to_string());
        let mut file = tokio::fs::File::open(path).await.map_err(unavailable)?;
        let size = file.metadata().await.map_err(unavailable)?.len();
        let start = offset.min(size);
        let end = start
            .saturating_add(length.unwrap_or(MAX_SLICE).min(MAX_SLICE))
            .min(size);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(unavailable)?;
        // Grown as read: `take` bounds it.
        let mut bytes = Vec::new();
        file.take(end - start)
            .read_to_end(&mut bytes)
            .await
            .map_err(unavailable)?;
        Ok((bytes, size))
    }

    /// Takes `bytes` as part `part` of `parts` of what the compile `id` made,
    /// whose SHA-256 is `sha256`, deflated when `deflated`: answers whether it
    /// is now whole.
    #[allow(clippy::too_many_arguments)]
    pub fn hand_back(
        &self,
        id: &str,
        tenant: &str,
        worker: &str,
        sha256: &str,
        part: u32,
        parts: u32,
        deflated: bool,
        bytes: Vec<u8>,
    ) -> Result<bool, Refusal> {
        if parts == 0 || part >= parts {
            return Err(Refusal::BadPart("not a part of the artifact"));
        }
        let job = self.taken(id, tenant, worker)?;
        let mut made = job.made.lock().expect("compile output poisoned");
        let expected = (sha256.to_owned(), parts);
        if made.expected.get_or_insert_with(|| expected.clone()) != &expected {
            return Err(Refusal::BadPart(
                "this compile is handing back another artifact",
            ));
        }
        if made.whole.is_some() {
            return Ok(true);
        }
        if made.parts.values().map(Vec::len).sum::<usize>() + bytes.len() > MADE_LIMIT {
            return Err(Refusal::BadPart("the artifact is over the limit"));
        }
        made.parts.insert(part, bytes);
        if made.parts.len() < parts as usize {
            return Ok(false);
        }
        let mut whole: Vec<u8> = std::mem::take(&mut made.parts)
            .into_values()
            .flatten()
            .collect();
        if deflated {
            use std::io::Read;
            let mut inflated = Vec::with_capacity(whole.len() * 3);
            let read = flate2::read::DeflateDecoder::new(whole.as_slice())
                .take(MADE_LIMIT as u64 + 1)
                .read_to_end(&mut inflated);
            if read.is_err() || inflated.len() > MADE_LIMIT {
                made.expected = None;
                return Err(Refusal::BadPart("the parts do not inflate to an artifact"));
            }
            whole = inflated;
        }
        let digest: String = sha2::Sha256::digest(&whole)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if digest != sha256 {
            made.expected = None;
            return Err(Refusal::BadPart("the parts are not the artifact they name"));
        }
        made.whole = Some(whole);
        Ok(true)
    }

    /// Ends the compile `id` with `outcome`, and what it handed back.
    pub fn finish(
        &self,
        id: &str,
        tenant: &str,
        worker: &str,
        outcome: Vec<u8>,
    ) -> Result<(), Refusal> {
        let job = self.taken(id, tenant, worker)?;
        self.state
            .lock()
            .expect("compile queues poisoned")
            .jobs
            .remove(id);
        let made = job
            .made
            .lock()
            .expect("compile output poisoned")
            .whole
            .take();
        if let Some(done) = job.done.lock().expect("compile job poisoned").take() {
            let _ = done.send(CompileResult { outcome, made });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(bytes: &[u8]) -> String {
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[tokio::test]
    async fn a_worker_takes_its_tenants_compiles_in_order_and_hands_each_back() {
        let dir = tempfile::tempdir().unwrap();
        let component = dir.path().join("stdlib.wasm");
        std::fs::write(&component, b"stdlib").unwrap();
        let queues = Arc::new(CompileQueues::default());
        let components: BTreeMap<_, _> = [(digest(b"stdlib"), component)].into();

        // A worker already waiting is woken by a compile queued after it asked.
        let waiting = {
            let queues = queues.clone();
            tokio::spawn(async move { queues.next("acme", "w1", Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (first, first_done) = queues.submit("acme", b"one".to_vec(), components.clone());
        let (second, _second_done) = queues.submit("acme", b"two".to_vec(), components.clone());
        let (id, request) = waiting.await.unwrap().unwrap();
        assert_eq!(
            (id.as_str(), request.as_slice()),
            (first.as_str(), b"one".as_slice())
        );

        // Another tenant's worker is served none of them.
        assert!(
            queues
                .next("beta", "w2", Duration::from_millis(20))
                .await
                .is_none()
        );
        let sha = digest(b"stdlib");
        assert_eq!(
            queues.artifact(&first, "beta", "w2", &sha, 0, None).await,
            Err(Refusal::NotYours)
        );
        // Nor is another worker of the same tenant served a compile it did not take.
        assert_eq!(
            queues.artifact(&first, "acme", "w3", &sha, 0, None).await,
            Err(Refusal::NotYours)
        );
        assert_eq!(
            queues
                .artifact(&first, "acme", "w1", &digest(b"other"), 0, None)
                .await,
            Err(Refusal::NotAComponent)
        );
        let (slice, size) = queues
            .artifact(&first, "acme", "w1", &sha, 2, Some(3))
            .await
            .unwrap();
        assert_eq!((slice.as_slice(), size), (b"dli".as_slice(), 6));

        let made = b"composed component";
        let made_sha = digest(made);
        assert!(
            !queues
                .hand_back(
                    &first,
                    "acme",
                    "w1",
                    &made_sha,
                    1,
                    2,
                    false,
                    made[9..].to_vec()
                )
                .unwrap()
        );
        assert!(
            queues
                .hand_back(
                    &first,
                    "acme",
                    "w1",
                    &made_sha,
                    0,
                    2,
                    false,
                    made[..9].to_vec()
                )
                .unwrap()
        );
        queues
            .finish(&first, "acme", "w1", b"compiled".to_vec())
            .unwrap();
        let result = first_done.await.unwrap();
        assert_eq!(result.outcome, b"compiled");
        assert_eq!(result.made.as_deref(), Some(made.as_slice()));
        // Ended, it is served no more.
        assert_eq!(
            queues.finish(&first, "acme", "w1", b"again".to_vec()),
            Err(Refusal::NotYours)
        );

        let (id, request) = queues
            .next("acme", "w1", Duration::from_millis(20))
            .await
            .unwrap();
        assert_eq!(
            (id.as_str(), request.as_slice()),
            (second.as_str(), b"two".as_slice())
        );
    }

    #[tokio::test]
    async fn parts_that_do_not_make_the_named_component_are_refused_and_a_cancelled_compile_ends() {
        let queues = CompileQueues::default();
        let (id, done) = queues.submit("acme", b"one".to_vec(), BTreeMap::new());
        queues
            .next("acme", "w1", Duration::from_millis(20))
            .await
            .unwrap();
        let wrong = digest(b"something else");
        assert!(
            queues
                .hand_back(&id, "acme", "w1", &wrong, 0, 1, false, b"made".to_vec())
                .is_err()
        );
        assert!(
            queues
                .hand_back(&id, "acme", "w1", &wrong, 3, 1, false, b"made".to_vec())
                .is_err()
        );
        // Deflated parts are taken once they inflate to the component named.
        let made = vec![7u8; 100_000];
        let deflated = {
            use std::io::Write;
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&made).unwrap();
            encoder.finish().unwrap()
        };
        assert!(
            queues
                .hand_back(&id, "acme", "w1", &digest(&made), 0, 1, true, deflated)
                .unwrap()
        );
        queues.cancel(&id);
        assert!(done.await.is_err(), "a cancelled compile's result ends");
        assert_eq!(
            queues.finish(&id, "acme", "w1", b"late".to_vec()),
            Err(Refusal::NotYours)
        );
    }
}
