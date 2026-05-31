// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Static child-workflow metadata for direct workflow artifacts.
//!
//! `EmbedWorkflow` steps inline a child graph *into* the parent component rather
//! than linking it at runtime, so the set of preloaded children must match what
//! the graph references at compile time. This cross-checks that closure — every
//! embedded step has a child whose id/version agree, no duplicates — and rebuilds
//! each child's manifest to record its checksum and feature summary into the
//! artifact sidecar (provenance + cache invalidation). A mismatch fails fast here
//! instead of surfacing later as an obscure `wac compose` error.

use std::collections::BTreeMap;

use crate::compile::ChildWorkflowInput;
use crate::workflow_features::WorkflowFeatureSummary;

use super::error::DirectCompileError;
use super::manifest::{DirectWorkflowManifest, build_direct_workflow_manifest};

/// One preloaded child workflow captured in direct artifact metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectChildWorkflowDependencyMetadata {
    /// `EmbedWorkflow` step id that references this child.
    pub step_id: String,
    /// Referenced child workflow id.
    pub workflow_id: String,
    /// Version requested by the parent DSL, such as `latest` or `2`.
    pub version_requested: String,
    /// Version resolved by the caller before compilation.
    pub version_resolved: i32,
    /// SHA-256 checksum of the child's direct manifest.
    pub manifest_checksum: String,
    /// Direct-emitter feature summary for the child graph.
    pub feature_summary: WorkflowFeatureSummary,
}

pub(super) fn resolve_direct_child_workflow_metadata(
    manifest: &DirectWorkflowManifest,
    child_workflows: &[ChildWorkflowInput],
) -> Result<Vec<DirectChildWorkflowDependencyMetadata>, DirectCompileError> {
    if manifest.feature_summary.child_workflows.is_empty() {
        return Ok(Vec::new());
    }

    let by_step_id = child_workflows_by_step_id(child_workflows)?;
    for reference in &manifest.feature_summary.child_workflows {
        let Some(child) = by_step_id.get(reference.step_id.as_str()) else {
            return Err(DirectCompileError::Component(format!(
                "direct child workflow metadata missing preloaded child for EmbedWorkflow step '{}'",
                reference.step_id
            )));
        };
        if child.workflow_id != reference.workflow_id {
            return Err(DirectCompileError::Component(format!(
                "direct child workflow metadata for step '{}' resolved workflow '{}' but graph references '{}'",
                reference.step_id, child.workflow_id, reference.workflow_id
            )));
        }
        if child.version_requested != reference.version {
            return Err(DirectCompileError::Component(format!(
                "direct child workflow metadata for step '{}' resolved requested version '{}' but graph references '{}'",
                reference.step_id, child.version_requested, reference.version
            )));
        }
    }

    let mut dependencies = Vec::with_capacity(child_workflows.len());
    for child in child_workflows {
        let child_manifest = build_direct_workflow_manifest(&child.execution_graph)?;
        dependencies.push(DirectChildWorkflowDependencyMetadata {
            step_id: child.step_id.clone(),
            workflow_id: child.workflow_id.clone(),
            version_requested: child.version_requested.clone(),
            version_resolved: child.version_resolved,
            manifest_checksum: child_manifest.checksum().to_string(),
            feature_summary: child_manifest.feature_summary,
        });
    }
    dependencies.sort_by(|left, right| {
        (
            left.step_id.as_str(),
            left.workflow_id.as_str(),
            left.version_resolved,
        )
            .cmp(&(
                right.step_id.as_str(),
                right.workflow_id.as_str(),
                right.version_resolved,
            ))
    });

    Ok(dependencies)
}

fn child_workflows_by_step_id(
    child_workflows: &[ChildWorkflowInput],
) -> Result<BTreeMap<&str, &ChildWorkflowInput>, DirectCompileError> {
    let mut by_step_id = BTreeMap::new();
    for child in child_workflows {
        if by_step_id.insert(child.step_id.as_str(), child).is_some() {
            return Err(DirectCompileError::Component(format!(
                "direct child workflow metadata received duplicate preloaded child for EmbedWorkflow step '{}'",
                child.step_id
            )));
        }
    }
    Ok(by_step_id)
}

#[cfg(test)]
mod tests {
    use runtara_dsl::ExecutionGraph;

    use super::*;

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "parent" => include_str!("../../tests/fixtures/embed_workflow_workflow.json"),
            "child" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn direct_child_workflow_metadata_records_static_preloaded_closure() {
        let parent = build_direct_workflow_manifest(&fixture("parent")).expect("parent manifest");
        let child_graph = fixture("child");
        let expected_child = build_direct_workflow_manifest(&child_graph).expect("child manifest");

        let metadata = resolve_direct_child_workflow_metadata(
            &parent,
            &[ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: child_graph,
            }],
        )
        .expect("child metadata");

        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].step_id, "call_child");
        assert_eq!(metadata[0].workflow_id, "child_workflow");
        assert_eq!(metadata[0].version_requested, "latest");
        assert_eq!(metadata[0].version_resolved, 3);
        assert_eq!(metadata[0].manifest_checksum, expected_child.checksum());
        assert_eq!(metadata[0].feature_summary, expected_child.feature_summary);
    }

    #[test]
    fn direct_child_workflow_metadata_rejects_missing_preloaded_child() {
        let parent = build_direct_workflow_manifest(&fixture("parent")).expect("parent manifest");

        let err = resolve_direct_child_workflow_metadata(&parent, &[])
            .expect_err("missing child should be rejected");
        let DirectCompileError::Component(message) = err else {
            panic!("expected component error");
        };
        assert!(message.contains("call_child"));
        assert!(message.contains("missing preloaded child"));
    }

    #[test]
    fn direct_child_workflow_metadata_ignores_unused_preloads_without_embed_steps() {
        let parent = build_direct_workflow_manifest(&fixture("child")).expect("parent manifest");

        let metadata = resolve_direct_child_workflow_metadata(
            &parent,
            &[ChildWorkflowInput {
                step_id: "unused".to_string(),
                workflow_id: "child".to_string(),
                version_requested: "1".to_string(),
                version_resolved: 1,
                execution_graph: fixture("child"),
            }],
        )
        .expect("unused child metadata");

        assert!(metadata.is_empty());
    }
}
