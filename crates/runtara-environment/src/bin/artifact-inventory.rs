// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Answer the G9 question against a real deployment: is any registered or
//! parked artifact still a legacy isolation package?
//!
//! The isolation package decoder, prepared child catalog, scoped runtime host
//! and legacy runner are retained purely so artifacts compiled before standard
//! composition keep executing and replaying. Nothing in the repository can say
//! whether such artifacts still exist — that is a property of a deployment, not
//! of the source — so retirement has been blocked on an inventory nobody could
//! run.
//!
//! This reads `images` and `instance_images`, classifies every artifact on disk
//! by whether it decodes as an isolation package, and reports which legacy
//! artifacts still back instances that are not terminal. A parked instance is
//! the sharp case: it will be relaunched later and must find the machinery its
//! artifact needs.
//!
//! Exit status is the gate. `0` means no legacy artifact backs a live or parked
//! instance, so the machinery can be retired. `1` means it cannot yet, and the
//! report says exactly what is holding it.
//!
//! ```sh
//! DATABASE_URL=postgres://... cargo run -p runtara-environment --bin artifact-inventory
//! DATABASE_URL=postgres://... cargo run -p runtara-environment --bin artifact-inventory -- --json
//! ```
use std::collections::BTreeMap;

use runtara_workflow_wit::isolation_package::{PackageLimits, parse};

/// Generous enough to decode any artifact this fleet could have produced:
/// misjudging a large legacy package as unreadable would understate the risk.
fn limits() -> PackageLimits {
    PackageLimits {
        total_bytes: 512 * 1024 * 1024,
        manifest_bytes: 16 * 1024 * 1024,
        artifacts: 16 * 1024,
        bindings: 16 * 1024,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    /// Decodes as an isolation package: needs the retained machinery.
    Legacy,
    /// Standard component composition.
    Standard,
    /// On disk but not decodable either way — reported, never assumed safe.
    Unreadable,
    /// Referenced by the database with no file behind it.
    Missing,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Legacy => "legacy-isolation-package",
            Shape::Standard => "standard-composition",
            Shape::Unreadable => "unreadable",
            Shape::Missing => "missing-file",
        }
    }
}

#[derive(sqlx::FromRow)]
struct ImageRow {
    image_id: String,
    tenant_id: String,
    name: String,
    binary_path: String,
    /// Instances that could still run this artifact again.
    live_instances: i64,
    /// Of those, the ones already parked — they WILL be relaunched.
    parked_instances: i64,
    total_instances: i64,
}

fn classify(path: &str) -> Shape {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Shape::Missing,
    };
    match parse(&bytes, limits()) {
        Ok(Some(_)) => Shape::Legacy,
        // A well-formed artifact that is not a package is standard composition.
        Ok(None) => Shape::Standard,
        Err(_) => Shape::Unreadable,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json = std::env::args().any(|arg| arg == "--json");
    let url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
        .map_err(|_| "set DATABASE_URL to the environment database")?;
    let pool = sqlx::PgPool::connect(&url).await?;

    // `suspended` is counted separately because a parked instance is the one
    // that is guaranteed to need its artifact again.
    let rows: Vec<ImageRow> = sqlx::query_as(
        r#"
        SELECT i.image_id,
               i.tenant_id,
               i.name,
               i.binary_path,
               COALESCE(COUNT(*) FILTER (
                   WHERE inst.status NOT IN ('completed', 'failed', 'cancelled')
               ), 0) AS live_instances,
               COALESCE(COUNT(*) FILTER (WHERE inst.status = 'suspended'), 0) AS parked_instances,
               COALESCE(COUNT(inst.instance_id), 0) AS total_instances
        FROM images i
        LEFT JOIN instance_images ii ON ii.image_id = i.image_id
        LEFT JOIN instances inst ON inst.instance_id = ii.instance_id
        GROUP BY i.image_id, i.tenant_id, i.name, i.binary_path
        ORDER BY i.tenant_id, i.name
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut blocking = Vec::new();
    let mut entries = Vec::new();
    for row in &rows {
        let shape = classify(&row.binary_path);
        *counts.entry(shape.label()).or_default() += 1;
        // Unreadable counts as blocking: it cannot be shown safe.
        let holds_back = matches!(shape, Shape::Legacy | Shape::Unreadable)
            && (row.live_instances > 0 || row.parked_instances > 0);
        if holds_back {
            blocking.push((row, shape));
        }
        entries.push(serde_json::json!({
            "imageId": row.image_id,
            "tenantId": row.tenant_id,
            "name": row.name,
            "shape": shape.label(),
            "liveInstances": row.live_instances,
            "parkedInstances": row.parked_instances,
            "totalInstances": row.total_instances,
            "holdsBackRetirement": holds_back,
        }));
    }

    let retirable = blocking.is_empty();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "images": entries,
                "counts": counts,
                "retirable": retirable,
            })
        );
    } else {
        println!("artifacts: {}", rows.len());
        for (label, count) in &counts {
            println!("  {label}: {count}");
        }
        if retirable {
            println!(
                "\nNo legacy or unreadable artifact backs a live or parked instance.\n\
                 The isolation decoder, prepared child catalog, scoped runtime host and legacy\n\
                 runner can be retired for this deployment."
            );
        } else {
            println!("\nRetirement is blocked by {} artifact(s):", blocking.len());
            for (row, shape) in &blocking {
                println!(
                    "  {} / {} ({}) — {} live, {} parked",
                    row.tenant_id,
                    row.name,
                    shape.label(),
                    row.live_instances,
                    row.parked_instances
                );
            }
        }
    }

    if retirable {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Shape, classify};

    /// The gate is only as good as this classifier: calling a standard artifact
    /// `legacy` would block retirement forever, and calling a legacy one
    /// `standard` would green-light deleting machinery a parked instance needs.
    #[test]
    fn a_real_component_is_recognised_as_standard_composition() {
        let Some(dir) = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR") else {
            eprintln!("skipped: set RUNTARA_AGENT_COMPONENTS_DIR to a staged component build");
            return;
        };
        let component = std::path::Path::new(&dir).join("runtara_agent_utils.wasm");
        if !component.exists() {
            eprintln!("skipped: {} is not staged", component.display());
            return;
        }
        assert_eq!(
            classify(component.to_str().expect("utf-8 path")),
            Shape::Standard,
            "an ordinary component must not be mistaken for an isolation package"
        );
    }

    #[test]
    fn a_path_with_no_file_is_missing_rather_than_readable() {
        assert_eq!(
            classify("/nonexistent/artifact-inventory/probe.wasm"),
            Shape::Missing
        );
    }

    /// Anything that is not a decodable artifact must land in a bucket the gate
    /// treats as blocking, never silently as standard.
    #[test]
    fn a_non_wasm_file_is_unreadable_not_standard() {
        let file = std::env::temp_dir().join("artifact-inventory-not-wasm.bin");
        std::fs::write(&file, b"this is not a wasm module").expect("fixture write");
        let shape = classify(file.to_str().expect("utf-8 path"));
        let _ = std::fs::remove_file(&file);
        assert_eq!(shape, Shape::Unreadable);
    }
}
