// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the engine has approved to run, which is what makes a workflow
//! runnable.
//!
//! A workflow's definition is a record the API keeps, in the server database,
//! and the API's process may be broken into. So the engine does not run a
//! definition because it is there. It runs the version of a workflow someone
//! with the authority to run it asked to have compiled or run, as the
//! definition was when they asked, from the image the engine itself compiled
//! from it; and where a run names no version, the one that was chosen through
//! the engine. Both are kept in the runtime database, which is the engine's
//! alone.
//!
//! A definition changed behind the engine's back therefore does not run: a
//! run that no caller asked for (a schedule, a webhook) finds that it is not
//! what was approved, and is refused until someone with the authority
//! compiles or runs it.

use std::sync::OnceLock;

use sqlx::{PgPool, Row};

/// A version of a workflow as its definition stands in the server database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub checksum: String,
    pub track_events: bool,
}

/// What was approved of one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub source: Source,
    /// The image the engine compiled from it, once it has.
    pub image_id: Option<String>,
}

/// What the engine does with a version that is about to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The definition is the one approved, and so is the image it would run
    /// from.
    Run,
    /// The definition is the one approved, and has no image of the engine's
    /// yet (or not this one): it may be compiled, and is not run until it is.
    Compile,
    /// The definition is not the one approved.
    Refuse,
}

/// The verdict on a version whose definition is `source`, approved as
/// `approval`, and which the server database says is compiled to `image`.
pub fn verdict(source: &Source, approval: Option<&Approval>, image: Option<&str>) -> Verdict {
    match approval {
        Some(approval) if approval.source == *source => match (image, &approval.image_id) {
            (Some(image), Some(approved)) if image == approved => Verdict::Run,
            _ => Verdict::Compile,
        },
        _ => Verdict::Refuse,
    }
}

/// Why a version is refused, as its run is told.
pub const NOT_APPROVED: &str = "this version's definition is not the one that was approved to \
     run: it was changed, or never compiled, since. Compile or run it as someone who may";

/// The engine's records of what may run.
pub struct Approvals {
    /// The runtime database, where they are kept.
    runtime: PgPool,
    /// The server database, where definitions are read.
    server: PgPool,
}

static INSTALLED: OnceLock<Approvals> = OnceLock::new();

/// The records of the engine in this process. `None` in a process with no
/// runtime database, which runs nothing.
pub fn installed() -> Option<&'static Approvals> {
    INSTALLED.get()
}

/// Makes `approvals` the records of the engine in this process.
pub fn install(approvals: Approvals) -> &'static Approvals {
    INSTALLED.get_or_init(|| approvals)
}

tokio::task_local! {
    static ASKER: String;
}

/// Runs `work` as asked by the caller `who`, whom this process (or, for an
/// engine in a process of its own, the engine) authenticated. What a caller
/// asks to have compiled or run is approved as it asks; what nobody asked
/// (a schedule, a webhook) approves nothing.
pub async fn asked_by<F: std::future::Future>(who: impl Into<String>, work: F) -> F::Output {
    ASKER.scope(who.into(), work).await
}

/// The caller who asked for the current work.
pub fn asker() -> Option<String> {
    ASKER.try_with(Clone::clone).ok()
}

/// Approves a version as its definition stands, where a caller asked for the
/// current work and the engine keeps records in this process.
pub async fn approve_as_asked(
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
) -> Result<(), String> {
    match (installed(), asker()) {
        (Some(approvals), Some(by)) => {
            approvals
                .approve(tenant_id, workflow_id, version, &by)
                .await
        }
        _ => Ok(()),
    }
}

type Failed = String;

fn failed(error: sqlx::Error) -> Failed {
    error.to_string()
}

impl Approvals {
    pub fn new(runtime: PgPool, server: PgPool) -> Self {
        Self { runtime, server }
    }

    /// The definition of a version as it stands. `None` if there is none.
    pub async fn source(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<Source>, Failed> {
        let row = sqlx::query(
            "SELECT definition, track_events FROM workflow_definitions \
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.server)
        .await
        .map_err(failed)?;
        Ok(row.map(|row| Source {
            checksum: crate::api::repositories::workflows::workflow_definition_checksum(
                &row.get::<serde_json::Value, _>("definition"),
            ),
            track_events: row.get("track_events"),
        }))
    }

    /// What was approved of a version.
    pub async fn approval(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<Approval>, Failed> {
        let row = sqlx::query(
            "SELECT source_checksum, track_events, image_id FROM workflow_approvals \
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.runtime)
        .await
        .map_err(failed)?;
        Ok(row.map(|row| Approval {
            source: Source {
                checksum: row.get("source_checksum"),
                track_events: row.get("track_events"),
            },
            image_id: row.get("image_id"),
        }))
    }

    /// Approves a version as its definition now stands, for `by`, who asked
    /// to have it compiled or run and may. An image the engine compiled from
    /// the same definition stays approved with it.
    pub async fn approve(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        by: &str,
    ) -> Result<(), Failed> {
        let Some(source) = self.source(tenant_id, workflow_id, version).await? else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO workflow_approvals \
                 (tenant_id, workflow_id, version, source_checksum, track_events, approved_by) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (tenant_id, workflow_id, version) DO UPDATE SET \
                 image_id = CASE \
                     WHEN workflow_approvals.source_checksum = EXCLUDED.source_checksum \
                      AND workflow_approvals.track_events = EXCLUDED.track_events \
                     THEN workflow_approvals.image_id END, \
                 source_checksum = EXCLUDED.source_checksum, \
                 track_events = EXCLUDED.track_events, \
                 approved_by = EXCLUDED.approved_by, \
                 approved_at = now()",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .bind(&source.checksum)
        .bind(source.track_events)
        .bind(by)
        .execute(&self.runtime)
        .await
        .map_err(failed)?;
        Ok(())
    }

    /// Records the image the engine compiled from `source`, where that is
    /// the definition that was approved. A compile of anything else approves
    /// nothing.
    pub async fn compiled(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        source: &Source,
        image_id: &str,
    ) -> Result<(), Failed> {
        sqlx::query(
            "UPDATE workflow_approvals SET image_id = $6 \
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 \
               AND source_checksum = $4 AND track_events = $5",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .bind(&source.checksum)
        .bind(source.track_events)
        .bind(image_id)
        .execute(&self.runtime)
        .await
        .map_err(failed)?;
        Ok(())
    }

    /// What the engine does with a version the server database says is
    /// compiled to `image`.
    pub async fn verdict(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        image: Option<&str>,
    ) -> Result<Verdict, Failed> {
        let Some(source) = self.source(tenant_id, workflow_id, version).await? else {
            return Ok(Verdict::Refuse);
        };
        let approval = self.approval(tenant_id, workflow_id, version).await?;
        Ok(verdict(&source, approval.as_ref(), image))
    }

    /// Makes `version` the one that runs when a run names none.
    pub async fn set_current(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        by: &str,
    ) -> Result<(), Failed> {
        sqlx::query(
            "INSERT INTO workflow_current_versions (tenant_id, workflow_id, version, set_by) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (tenant_id, workflow_id) DO UPDATE SET \
                 version = EXCLUDED.version, set_by = EXCLUDED.set_by, set_at = now()",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .bind(by)
        .execute(&self.runtime)
        .await
        .map_err(failed)?;
        Ok(())
    }

    /// The version that was chosen to run when a run names none.
    pub async fn current(&self, tenant_id: &str, workflow_id: &str) -> Result<Option<i32>, Failed> {
        sqlx::query_scalar(
            "SELECT version FROM workflow_current_versions \
             WHERE tenant_id = $1 AND workflow_id = $2",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .fetch_optional(&self.runtime)
        .await
        .map_err(failed)
    }

    /// Carries over, once, what ran before the engine kept these records: a
    /// version compiled then is approved as it was compiled, and a version
    /// chosen then stays chosen. Without it nothing compiled before would
    /// run. Runs across tenants, as the platform's own work.
    pub async fn carry_over_once(&self) -> Result<Option<(u64, u64)>, Failed> {
        const NAME: &str = "what-ran-before-approvals";
        let mut runtime = self.runtime.begin().await.map_err(failed)?;
        let first = sqlx::query(
            "INSERT INTO engine_carried_over (name) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(NAME)
        .execute(&mut *runtime)
        .await
        .map_err(failed)?
        .rows_affected()
            == 1;
        if !first {
            return Ok(None);
        }

        let compiled = sqlx::query(
            "SELECT tenant_id, workflow_id, version, source_checksum, \
                    COALESCE(track_events, false) AS track_events, registered_image_id \
             FROM workflow_compilations \
             WHERE compilation_status = 'success' \
               AND source_checksum IS NOT NULL AND registered_image_id IS NOT NULL",
        )
        .fetch_all(&self.server)
        .await
        .map_err(failed)?;
        let mut approved = 0;
        for row in &compiled {
            approved += sqlx::query(
                "INSERT INTO workflow_approvals \
                     (tenant_id, workflow_id, version, source_checksum, track_events, image_id, \
                      approved_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, 'carried over') ON CONFLICT DO NOTHING",
            )
            .bind(row.get::<String, _>("tenant_id"))
            .bind(row.get::<String, _>("workflow_id"))
            .bind(row.get::<i32, _>("version"))
            .bind(row.get::<String, _>("source_checksum"))
            .bind(row.get::<bool, _>("track_events"))
            .bind(row.get::<String, _>("registered_image_id"))
            .execute(&mut *runtime)
            .await
            .map_err(failed)?
            .rows_affected();
        }

        let chosen = sqlx::query(
            "SELECT tenant_id, workflow_id, current_version FROM workflows \
             WHERE current_version IS NOT NULL AND deleted_at IS NULL",
        )
        .fetch_all(&self.server)
        .await
        .map_err(failed)?;
        let mut current = 0;
        for row in &chosen {
            current += sqlx::query(
                "INSERT INTO workflow_current_versions (tenant_id, workflow_id, version, set_by) \
                 VALUES ($1, $2, $3, 'carried over') ON CONFLICT DO NOTHING",
            )
            .bind(row.get::<String, _>("tenant_id"))
            .bind(row.get::<String, _>("workflow_id"))
            .bind(row.get::<i32, _>("current_version"))
            .execute(&mut *runtime)
            .await
            .map_err(failed)?
            .rows_affected();
        }
        runtime.commit().await.map_err(failed)?;
        Ok(Some((approved, current)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(checksum: &str) -> Source {
        Source {
            checksum: checksum.to_owned(),
            track_events: false,
        }
    }

    fn approved(checksum: &str, image: Option<&str>) -> Approval {
        Approval {
            source: source(checksum),
            image_id: image.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn a_caller_is_whoever_the_work_was_asked_by_and_no_one_after() {
        assert_eq!(asker(), None);
        assert_eq!(
            asked_by("ann", async { asker() }).await.as_deref(),
            Some("ann")
        );
        let spawned = asked_by("ann", async {
            tokio::spawn(async { asker() }).await.unwrap()
        });
        assert_eq!(spawned.await, None);
    }

    #[test]
    fn what_was_approved_runs_from_the_image_the_engine_compiled() {
        let approval = approved("a", Some("image-1"));
        assert_eq!(
            verdict(&source("a"), Some(&approval), Some("image-1")),
            Verdict::Run
        );
    }

    #[test]
    fn a_definition_changed_since_is_refused_whatever_it_is_compiled_to() {
        let approval = approved("a", Some("image-1"));
        for image in [None, Some("image-1"), Some("image-2")] {
            assert_eq!(
                verdict(&source("b"), Some(&approval), image),
                Verdict::Refuse
            );
        }
        // The same definition, tracked differently, is another program.
        let tracked = Source {
            checksum: "a".to_owned(),
            track_events: true,
        };
        assert_eq!(
            verdict(&tracked, Some(&approval), Some("image-1")),
            Verdict::Refuse
        );
    }

    #[test]
    fn what_was_never_approved_is_refused() {
        assert_eq!(verdict(&source("a"), None, None), Verdict::Refuse);
        assert_eq!(
            verdict(&source("a"), None, Some("image-1")),
            Verdict::Refuse
        );
    }

    #[test]
    fn an_image_the_engine_did_not_compile_from_it_is_not_run() {
        // Approved, and not compiled yet: it may be compiled.
        assert_eq!(
            verdict(&source("a"), Some(&approved("a", None)), None),
            Verdict::Compile
        );
        // The server database names an image that is not the one the engine
        // compiled from this definition: it is compiled, not run.
        let approval = approved("a", Some("image-1"));
        assert_eq!(
            verdict(&source("a"), Some(&approval), Some("another-image")),
            Verdict::Compile
        );
        assert_eq!(
            verdict(&source("a"), Some(&approved("a", None)), Some("image-1")),
            Verdict::Compile
        );
    }
}
