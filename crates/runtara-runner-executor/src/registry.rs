// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The executors a server has heard from.
//!
//! An executor says it is here on the run channel's listener
//! ([`router`]), at start and every [`HEARTBEAT`] after; the listener serves
//! only the executor's certificate, so nothing else can. The server keeps
//! what each last said, in memory, and places runs with those it heard from
//! within [`AWAY_AFTER`]. Nothing is configured: a server that started again
//! learns its executors from their next call.
//!
//! An address is not trusted for being sent. Runs are placed there over a
//! connection that requires the executor's certificate, so an address that is
//! not an executor of this deployment fails the handshake.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::routing::put;
use axum::{Extension, Json, Router};
use runtara_executor::protocol::{
    EXECUTOR_NAME, EXECUTORS_PATH, Refusal, Registered, Registration, valid_boot,
};
use runtara_executor::tls::{self, Identity, Peer};

/// How often an executor is told to call.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// An executor not heard from for this long is away: no run is placed with
/// it. The runs it holds are not lost by this; each is by its own status
/// call.
pub const AWAY_AFTER: Duration = Duration::from_secs(15);
/// An executor not heard from for this long is forgotten.
const FORGOTTEN_AFTER: Duration = Duration::from_secs(300);

/// An executor to place a run with, or to ask about one.
#[derive(Clone)]
pub struct Executor {
    /// Its id for as long as it runs.
    pub boot: String,
    /// `https://host:port` of its placement API.
    pub address: String,
    pub client: tls::Client,
}

struct Member {
    executor: Executor,
    max_runs: usize,
    /// Runs it held when it last called.
    running: usize,
    /// Runs this server placed with it since.
    placed: usize,
    seen: Instant,
    /// Not offered a run until then: it was full or did not answer.
    passed_over_until: Option<Instant>,
}

impl Member {
    fn free(&self) -> usize {
        self.max_runs.saturating_sub(self.running + self.placed)
    }
}

/// The executors heard from, by boot.
pub struct Registry {
    identity: Identity,
    members: Mutex<HashMap<String, Member>>,
    /// Whose turn it is among executors equally free.
    turn: AtomicUsize,
}

impl Registry {
    /// A registry that reaches its executors as `identity`.
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            members: Mutex::default(),
            turn: AtomicUsize::new(0),
        }
    }

    fn members(&self) -> std::sync::MutexGuard<'_, HashMap<String, Member>> {
        self.members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Executor `boot`, calling from `peer`, is here as `registration` says.
    pub fn register(
        &self,
        boot: &str,
        peer: IpAddr,
        registration: &Registration,
    ) -> Result<Registered, String> {
        self.register_at(Instant::now(), boot, peer, registration)
    }

    fn register_at(
        &self,
        now: Instant,
        boot: &str,
        peer: IpAddr,
        registration: &Registration,
    ) -> Result<Registered, String> {
        if !valid_boot(boot) {
            return Err("not an executor's boot id".to_owned());
        }
        if !(1..=1024).contains(&registration.max_runs) {
            return Err("max_runs must be 1 to 1024".to_owned());
        }
        let address = match &registration.address {
            Some(address) => {
                let address = address.trim_end_matches('/');
                let uri: http::Uri = address
                    .parse()
                    .map_err(|_| "address is not a URL".to_owned())?;
                let plain = uri.path_and_query().is_none_or(|path| path.as_str() == "/");
                if uri.scheme_str() != Some("https") || uri.host().is_none() || !plain {
                    return Err("address must be https://host:port".to_owned());
                }
                address.to_owned()
            }
            None => format!("https://{}", SocketAddr::new(peer, registration.port)),
        };

        let mut members = self.members();
        members.retain(|other, member| {
            // An executor that started again at an address: the one that
            // was there is gone, and its runs with it.
            let replaced = other != boot && member.executor.address == address;
            if replaced {
                tracing::info!(%address, gone = %other, here = %boot, "an executor started again");
            }
            !replaced && now.saturating_duration_since(member.seen) < FORGOTTEN_AFTER
        });
        match members.get_mut(boot) {
            Some(member) if member.executor.address == address => {
                member.max_runs = registration.max_runs;
                member.running = registration.running;
                member.placed = 0;
                member.seen = now;
            }
            _ => {
                let config = self.identity.client_config(&self.identity.ca_pem, None)?;
                let client = tls::client(config, EXECUTOR_NAME)?;
                tracing::info!(
                    %boot,
                    %address,
                    max_runs = registration.max_runs,
                    "an executor registered"
                );
                members.insert(
                    boot.to_owned(),
                    Member {
                        executor: Executor {
                            boot: boot.to_owned(),
                            address,
                            client,
                        },
                        max_runs: registration.max_runs,
                        running: registration.running,
                        placed: 0,
                        seen: now,
                        passed_over_until: None,
                    },
                );
            }
        }
        Ok(Registered {
            heartbeat_ms: HEARTBEAT.as_millis() as u64,
            away_after_ms: AWAY_AFTER.as_millis() as u64,
        })
    }

    /// Executor `boot` is stopping; whether it was known.
    pub fn remove(&self, boot: &str) -> bool {
        let removed = self.members().remove(boot).is_some();
        if removed {
            tracing::info!(%boot, "an executor left");
        }
        removed
    }

    /// Executor `boot`, while it is known: away or not, since a run it holds
    /// is still asked about.
    pub fn get(&self, boot: &str) -> Option<Executor> {
        self.members()
            .get(boot)
            .map(|member| member.executor.clone())
    }

    /// The executors a run may be placed with now, the one to try first
    /// first: those with the most room, turns taken among equals.
    pub fn candidates(&self) -> Vec<Executor> {
        self.candidates_at(Instant::now())
    }

    fn candidates_at(&self, now: Instant) -> Vec<Executor> {
        let members = self.members();
        let mut offered: Vec<&Member> = members
            .values()
            .filter(|member| now.saturating_duration_since(member.seen) < AWAY_AFTER)
            .filter(|member| member.passed_over_until.is_none_or(|until| now >= until))
            .collect();
        if offered.is_empty() {
            return Vec::new();
        }
        offered.sort_by(|a, b| a.executor.boot.cmp(&b.executor.boot));
        let turn = self.turn.fetch_add(1, Ordering::Relaxed) % offered.len();
        offered.rotate_left(turn);
        // Stable: equals keep the order their turn gave them.
        offered.sort_by_key(|member| std::cmp::Reverse(member.free()));
        offered
            .into_iter()
            .map(|member| member.executor.clone())
            .collect()
    }

    /// One more run was placed with `boot`.
    pub fn placed(&self, boot: &str) {
        if let Some(member) = self.members().get_mut(boot) {
            member.placed += 1;
        }
    }

    /// `boot` is offered no run for `this_long`: it was full, or did not
    /// answer.
    pub fn pass_over(&self, boot: &str, this_long: Duration) {
        if let Some(member) = self.members().get_mut(boot) {
            member.passed_over_until = Some(Instant::now() + this_long);
        }
    }

    /// How many executors a run may be placed with now.
    pub fn active(&self) -> usize {
        let now = Instant::now();
        self.members()
            .values()
            .filter(|member| now.saturating_duration_since(member.seen) < AWAY_AFTER)
            .count()
    }
}

fn refuse(status: http::StatusCode, message: String) -> axum::response::Response {
    (
        status,
        Json(Refusal {
            error: "invalid".to_owned(),
            message,
        }),
    )
        .into_response()
}

/// Where executors say they are here and that they are leaving, on the run
/// channel's listener: `PUT` and `DELETE /executor/v1/executors/{boot}`.
pub fn router(registry: Arc<Registry>) -> Router {
    let here = {
        let registry = registry.clone();
        move |Path(boot): Path<String>,
              Extension(Peer(from)): Extension<Peer>,
              Json(registration): Json<Registration>| {
            let registry = registry.clone();
            async move {
                match registry.register(&boot, from.ip(), &registration) {
                    Ok(registered) => Json(registered).into_response(),
                    Err(message) => refuse(http::StatusCode::BAD_REQUEST, message),
                }
            }
        }
    };
    let leaving = move |Path(boot): Path<String>| {
        let registry = registry.clone();
        async move {
            registry.remove(&boot);
            http::StatusCode::NO_CONTENT
        }
    };
    Router::new().route(
        &format!("{EXECUTORS_PATH}/{{boot}}"),
        put(here).delete(leaving),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        let (for_server, _) = runtara_executor::pki::issue().unwrap();
        Registry::new(
            Identity::from_pem(
                for_server.ca_pem,
                for_server.certificate_pem.as_bytes(),
                for_server.key_pem.as_bytes(),
            )
            .unwrap(),
        )
    }

    fn at(port: u16, max_runs: usize, running: usize) -> Registration {
        Registration {
            address: None,
            port,
            max_runs,
            running,
        }
    }

    fn peer() -> IpAddr {
        "10.0.0.7".parse().unwrap()
    }

    fn boots(executors: Vec<Executor>) -> Vec<String> {
        executors.into_iter().map(|e| e.boot).collect()
    }

    #[tokio::test]
    async fn an_executor_is_reached_where_its_call_came_from_unless_it_says_otherwise() {
        let registry = registry();
        registry.register("a", peer(), &at(7443, 4, 0)).unwrap();
        assert_eq!(registry.get("a").unwrap().address, "https://10.0.0.7:7443");
        registry
            .register("b", "fd00::7".parse().unwrap(), &at(7443, 4, 0))
            .unwrap();
        assert_eq!(registry.get("b").unwrap().address, "https://[fd00::7]:7443");
        let mut named = at(7443, 4, 0);
        named.address = Some("https://executor-3.internal:9000/".to_owned());
        registry.register("c", peer(), &named).unwrap();
        assert_eq!(
            registry.get("c").unwrap().address,
            "https://executor-3.internal:9000"
        );
        for refused in ["http://plain:1", "https://host/path", "not a url"] {
            named.address = Some(refused.to_owned());
            assert!(registry.register("d", peer(), &named).is_err(), "{refused}");
        }
        assert!(registry.register("a:b", peer(), &at(1, 4, 0)).is_err());
        assert!(registry.register("e", peer(), &at(1, 0, 0)).is_err());
    }

    #[tokio::test]
    async fn the_executor_with_the_most_room_is_tried_first_and_equals_take_turns() {
        let registry = registry();
        registry.register("busy", peer(), &at(1, 4, 3)).unwrap();
        registry.register("idle", peer(), &at(2, 4, 0)).unwrap();
        assert_eq!(boots(registry.candidates()), ["idle", "busy"]);
        // What this server placed since counts until the executor says.
        for _ in 0..4 {
            registry.placed("idle");
        }
        assert_eq!(boots(registry.candidates()), ["busy", "idle"]);
        registry.register("idle", peer(), &at(2, 4, 1)).unwrap();
        assert_eq!(boots(registry.candidates()), ["idle", "busy"]);

        let registry = self::registry();
        registry.register("a", peer(), &at(1, 4, 0)).unwrap();
        registry.register("b", peer(), &at(2, 4, 0)).unwrap();
        let first: Vec<String> = (0..4)
            .map(|_| boots(registry.candidates()).remove(0))
            .collect();
        assert_eq!(first, ["a", "b", "a", "b"]);
    }

    #[tokio::test]
    async fn an_executor_not_heard_from_is_away_and_then_forgotten() {
        let registry = registry();
        let start = Instant::now();
        registry
            .register_at(start, "a", peer(), &at(1, 4, 0))
            .unwrap();
        assert_eq!(boots(registry.candidates_at(start + AWAY_AFTER / 2)), ["a"]);
        assert!(registry.candidates_at(start + AWAY_AFTER).is_empty());
        // Away, yet known: a run it holds is still asked about.
        assert!(registry.get("a").is_some());
        // Heard from again.
        registry
            .register_at(start + AWAY_AFTER * 2, "a", peer(), &at(1, 4, 0))
            .unwrap();
        assert_eq!(boots(registry.candidates_at(start + AWAY_AFTER * 2)), ["a"]);
        // Another's call, long after: the silent one is forgotten.
        registry
            .register_at(start + FORGOTTEN_AFTER * 2, "b", peer(), &at(2, 4, 0))
            .unwrap();
        assert!(registry.get("a").is_none());
    }

    #[tokio::test]
    async fn an_executor_that_started_again_replaces_the_one_at_its_address() {
        let registry = registry();
        registry.register("old", peer(), &at(1, 4, 0)).unwrap();
        registry.register("other", peer(), &at(2, 4, 0)).unwrap();
        registry.register("new", peer(), &at(1, 4, 0)).unwrap();
        assert!(registry.get("old").is_none());
        assert!(registry.get("other").is_some());
        assert!(registry.get("new").is_some());
        assert!(registry.remove("new"));
        assert!(!registry.remove("new"));
        assert_eq!(registry.active(), 1);
    }

    #[tokio::test]
    async fn an_executor_passed_over_is_not_offered_a_run_for_a_while() {
        let registry = registry();
        registry.register("a", peer(), &at(1, 4, 0)).unwrap();
        registry.register("b", peer(), &at(2, 4, 0)).unwrap();
        registry.pass_over("a", Duration::from_secs(60));
        assert_eq!(boots(registry.candidates()), ["b"]);
        registry.pass_over("a", Duration::ZERO);
        assert_eq!(registry.candidates().len(), 2);
    }
}
