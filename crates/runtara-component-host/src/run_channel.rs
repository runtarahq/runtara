// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The run channel: the workflow API served to a guest's runner over a
//! network, authenticated by a per-launch bearer credential.
//!
//! A runner that executes a launch elsewhere (a fenders shim, a container, a
//! process reached over ssh) is handed the credential in its launch spec and
//! presents it on every call. The credential names one launch: it is valid
//! from when the launch begins until it finishes, and calls are held until the
//! launch's durable start is confirmed (the start gate), so a guest cannot act
//! before its launch is fenced. Only the credential's SHA-256 is kept.
//!
//! The run's end needs nothing of its own: a guest posts its outcome, and a
//! runner whose guest died without one posts a failed outcome on its behalf
//! with the same credential.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use runtara_workflow_api::Binding;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

/// How long a call waits for its launch's start to be confirmed.
pub const START_WAIT: Duration = Duration::from_secs(120);

/// Whether a launch's durable start has been confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartGate {
    /// Not yet: calls wait.
    Pending,
    /// Confirmed: calls are served.
    Open,
    /// Refused: the launch will not run, and calls are refused.
    Refused,
}

struct Entry {
    launch_id: String,
    binding: Binding,
    gate: watch::Sender<StartGate>,
    /// When the credential was first presented: the run's start.
    first_used: Option<std::time::Instant>,
}

/// The credentials of the launches currently served over the run channel,
/// by the SHA-256 of each.
#[derive(Default)]
pub struct RunCredentials {
    by_digest: Mutex<HashMap<[u8; 32], Entry>>,
}

fn digest(credential: &str) -> [u8; 32] {
    Sha256::digest(credential.as_bytes()).into()
}

impl RunCredentials {
    /// A new credential for `binding`'s launch, replacing any it had. Its
    /// start gate is pending.
    pub fn mint(&self, binding: Binding) -> String {
        // Two v4 UUIDs: 244 bits from the operating system's generator.
        let credential = format!(
            "rlc1_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut entries = self.by_digest.lock().expect("run credentials poisoned");
        entries.retain(|_, entry| entry.launch_id != binding.launch_id);
        entries.insert(
            digest(&credential),
            Entry {
                launch_id: binding.launch_id.clone(),
                binding,
                gate: watch::Sender::new(StartGate::Pending),
                first_used: None,
            },
        );
        credential
    }

    fn set_gate(&self, launch_id: &str, gate: StartGate) -> bool {
        let entries = self.by_digest.lock().expect("run credentials poisoned");
        let mut found = false;
        for entry in entries.values().filter(|e| e.launch_id == launch_id) {
            entry.gate.send_replace(gate);
            found = true;
        }
        found
    }

    /// Opens `launch_id`'s start gate: its durable start is confirmed.
    pub fn open(&self, launch_id: &str) -> bool {
        self.set_gate(launch_id, StartGate::Open)
    }

    /// Refuses `launch_id`'s start: its waiting and later calls are refused.
    pub fn refuse(&self, launch_id: &str) -> bool {
        self.set_gate(launch_id, StartGate::Refused)
    }

    /// Revokes `launch_id`'s credential; later calls with it are refused.
    pub fn revoke(&self, launch_id: &str) {
        self.by_digest
            .lock()
            .expect("run credentials poisoned")
            .retain(|_, entry| entry.launch_id != launch_id);
    }

    /// How many launches hold a credential.
    pub fn len(&self) -> usize {
        self.by_digest
            .lock()
            .expect("run credentials poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lookup(&self, credential: &str) -> Option<(Binding, watch::Receiver<StartGate>)> {
        self.by_digest
            .lock()
            .expect("run credentials poisoned")
            .get_mut(&digest(credential))
            .map(|entry| {
                entry.first_used.get_or_insert_with(std::time::Instant::now);
                (entry.binding.clone(), entry.gate.subscribe())
            })
    }

    /// When `launch_id`'s credential was first presented.
    pub fn first_used(&self, launch_id: &str) -> Option<std::time::Instant> {
        self.by_digest
            .lock()
            .expect("run credentials poisoned")
            .values()
            .find(|entry| entry.launch_id == launch_id)
            .and_then(|entry| entry.first_used)
    }

    /// The binding a request's credential names, once its launch's start is
    /// confirmed; otherwise the refusal to answer with.
    pub async fn authorize(&self, headers: &HeaderMap) -> Result<Binding, Response> {
        let refused = |status: StatusCode| status.into_response();
        let credential = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| refused(StatusCode::UNAUTHORIZED))?;
        let (binding, mut gate) = self
            .lookup(credential)
            .ok_or_else(|| refused(StatusCode::UNAUTHORIZED))?;
        let waited = tokio::time::timeout(START_WAIT, async {
            loop {
                match *gate.borrow_and_update() {
                    StartGate::Open => return Ok(()),
                    StartGate::Refused => return Err(StatusCode::FORBIDDEN),
                    StartGate::Pending => {}
                }
                // The sender goes with the credential: revoked while waiting.
                if gate.changed().await.is_err() {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
        })
        .await;
        match waited {
            Ok(Ok(())) => Ok(binding),
            Ok(Err(status)) => Err(refused(status)),
            Err(_) => {
                let mut response = refused(StatusCode::SERVICE_UNAVAILABLE);
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
                Err(response)
            }
        }
    }
}

/// Serves `inner` (a `/v1` workflow API router that takes its [`Binding`]
/// from the request's extensions) to requests carrying a current run
/// credential, with that credential's binding. The credential and every
/// reserved header are removed before `inner` sees the request.
pub fn credentialed(inner: axum::Router, credentials: Arc<RunCredentials>) -> axum::Router {
    inner.layer(axum::middleware::from_fn(
        move |mut request: Request, next: Next| {
            let credentials = credentials.clone();
            async move {
                let binding = match credentials.authorize(request.headers()).await {
                    Ok(binding) => binding,
                    Err(refusal) => return refusal,
                };
                let headers = request.headers_mut();
                let reserved: Vec<_> = headers
                    .keys()
                    .filter(|name| runtara_workflow_api::headers::is_stripped(name.as_str()))
                    .cloned()
                    .collect();
                for name in reserved {
                    headers.remove(name);
                }
                request.extensions_mut().insert(binding);
                next.run(request).await
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    fn binding(launch: &str) -> Binding {
        Binding {
            tenant_id: "t-1".into(),
            instance_id: "i-1".into(),
            launch_id: launch.into(),
            root_lease_epoch: 7,
        }
    }

    fn app(credentials: Arc<RunCredentials>) -> axum::Router {
        credentialed(
            axum::Router::new().route(
                "/v1/run",
                get(|request: Request| async move {
                    let binding = request.extensions().get::<Binding>().cloned().unwrap();
                    let leaked = request.headers().contains_key("authorization")
                        || request.headers().contains_key("runtara-binding-launch");
                    format!(
                        "{} {} {leaked}",
                        binding.launch_id, binding.root_lease_epoch
                    )
                }),
            ),
            credentials,
        )
    }

    async fn call(app: &axum::Router, credential: &str) -> (StatusCode, String) {
        let response = app
            .clone()
            .oneshot(
                Request::get("/v1/run")
                    .header("authorization", format!("Bearer {credential}"))
                    .header("runtara-binding-launch", "someone-else")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn a_credential_names_its_launch_once_its_start_is_confirmed() {
        let credentials = Arc::new(RunCredentials::default());
        let credential = credentials.mint(binding("launch-1"));
        let app = app(credentials.clone());
        // Held at the start gate until the start is confirmed.
        let waiting = tokio::spawn({
            let (app, credential) = (app.clone(), credential.clone());
            async move { call(&app, &credential).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        assert!(credentials.first_used("launch-1").is_some());
        assert!(credentials.open("launch-1"));
        assert_eq!(
            waiting.await.unwrap(),
            (StatusCode::OK, "launch-1 7 false".into())
        );
        assert_eq!(call(&app, &credential).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_revoked_replaced_and_refused_credentials_are_refused() {
        let credentials = Arc::new(RunCredentials::default());
        let app = app(credentials.clone());
        assert_eq!(call(&app, "rlc1_guessed").await.0, StatusCode::UNAUTHORIZED);

        let first = credentials.mint(binding("launch-1"));
        let second = credentials.mint(binding("launch-1"));
        credentials.open("launch-1");
        assert_eq!(call(&app, &first).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, &second).await.0, StatusCode::OK);

        // Revoked while a call waits at the gate.
        let other = credentials.mint(binding("launch-2"));
        let waiting = tokio::spawn({
            let (app, other) = (app.clone(), other.clone());
            async move { call(&app, &other).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        credentials.revoke("launch-2");
        assert_eq!(waiting.await.unwrap().0, StatusCode::UNAUTHORIZED);

        let refused = credentials.mint(binding("launch-3"));
        credentials.refuse("launch-3");
        assert_eq!(call(&app, &refused).await.0, StatusCode::FORBIDDEN);

        credentials.revoke("launch-1");
        assert_eq!(call(&app, &second).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(credentials.len(), 1);
    }
}
