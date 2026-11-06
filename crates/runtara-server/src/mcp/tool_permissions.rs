//! Maps each MCP tool to the [`Permission`] it needs, so a scoped API key's granular scope
//! gates the tool surface the same way it gates the REST API (SYN-939, requirement 4).
//!
//! **Why this exists even though enforcement is already covered.** Every tool does its work
//! through in-process REST calls on the `internal_router`, which carries the same `authorize`
//! layer — so a scoped key is *already* denied at call time by the route it hits. This map adds
//! two things on top:
//!
//! - a **call-time gate keyed on the tool name** (in [`crate::mcp::server`] `call_tool`), which
//!   fails fast with a clear message before dispatch and closes the gap for any tool that does
//!   not route through a permission-gated REST path; and
//! - **visibility filtering** in `list_tools`, so a scoped session is shown only the tools it can
//!   actually use.
//!
//! **Safety property.** This map is *advisory with an authoritative backstop*: the downstream
//! route gate still runs on every tool's in-process call. So a mapping that is too *lenient*
//! (classifies a write tool as a read) can only let a call reach the route gate, which then
//! denies it — never a real escalation. A mapping that is too *strict* only over-hides/over-
//! blocks a tool (a UX regression, caught by the `every_tool_is_classified` guard below). The
//! map therefore fails safe in both directions.

use axum::http::Method;

use crate::authz::{ApiKeyScope, Permission};

/// What a tool requires of the caller's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolAuthz {
    /// Static catalog/schema tools that touch no tenant data — always available.
    Metadata,
    /// The tool needs this permission; a scope must grant it (or be `Full`).
    Needs(Permission),
    /// A tool this map does not know (a new tool added without a classification). Treated as
    /// allowed at this advisory layer — the authoritative route gate still runs — but the
    /// `every_tool_is_classified` test fails so it never ships unmapped.
    Unknown,
}

/// The authorization a given MCP tool needs. The arms mirror the REST route each tool drives
/// (see `middleware::authorization::permission_for`); grouped by resource for auditing.
pub(crate) fn tool_authz(tool: &str) -> ToolAuthz {
    use Permission::*;
    match tool {
        // ── Metadata: static schemas and catalogs, no tenant data ───────────────────────
        "get_workflow_authoring_schema"
        | "get_step_type_schema"
        | "list_step_types"
        | "list_agents"
        | "get_agent"
        | "get_capability" => ToolAuthz::Metadata,

        // ── Workflow reads ──────────────────────────────────────────────────────────────
        "list_workflows"
        | "get_workflow"
        | "list_workflow_folders"
        | "summarize_workflow"
        | "get_workflow_metadata"
        | "get_workflow_slice"
        | "diff_workflow_versions"
        | "find_references"
        | "list_references"
        | "list_unmapped_inputs"
        | "list_edges"
        | "list_steps"
        | "get_step"
        | "get_step_edges"
        | "get_step_mappings"
        | "get_input_schema"
        | "get_output_schema"
        | "get_state_schema"
        | "get_signal_schema"
        | "list_variables"
        | "validate_graph"
        | "validate_mappings"
        | "preflight_compile" => ToolAuthz::Needs(WorkflowRead),

        // ── Workflow create ─────────────────────────────────────────────────────────────
        "create_workflow" => ToolAuthz::Needs(WorkflowCreate),

        // ── Workflow update: graph mutations, metadata, versioning, deploy ──────────────
        "update_workflow"
        | "compile_workflow"
        | "deploy_workflow"
        | "deploy_latest"
        | "set_current_version"
        | "set_workflow_slug"
        | "set_workflow_metadata"
        | "move_workflow"
        | "publish_workflow_agent"
        | "add_agent_step"
        | "add_step"
        | "remove_step"
        | "update_step"
        | "patch_step"
        | "connect_steps"
        | "disconnect_steps"
        | "set_entry_point"
        | "set_mapping"
        | "remove_mapping"
        | "set_input_schema"
        | "set_input_schema_field"
        | "remove_input_schema_field"
        | "set_output_schema"
        | "set_state_schema"
        | "set_variable"
        | "remove_variable"
        | "apply_graph_mutations" => ToolAuthz::Needs(WorkflowUpdate),

        // ── Workflow delete ─────────────────────────────────────────────────────────────
        "delete_workflow" => ToolAuthz::Needs(WorkflowDelete),

        // ── Workflow execute: runs + lifecycle + answering a running run's waits ─────────
        "execute_workflow"
        | "execute_workflow_sync"
        | "execute_workflow_wait"
        | "stop_execution"
        | "pause_execution"
        | "resume_execution"
        | "submit_signal_response"
        | "submit_action_response"
        | "test_capability" => ToolAuthz::Needs(WorkflowExecute),

        // ── Invocation history reads (executions, steps, pending signals) ───────────────
        "list_executions"
        | "get_execution"
        | "get_step_events"
        | "get_step_summaries"
        | "inspect_step"
        | "why_execution_failed"
        | "trace_reference"
        | "list_pending_signals" => ToolAuthz::Needs(InvocationHistoryRead),

        // ── Object model / database ─────────────────────────────────────────────────────
        "list_object_schemas"
        | "get_object_schema"
        | "list_object_instances"
        | "query_object_instances"
        | "query_aggregate"
        | "query_sql"
        | "query_sql_one"
        | "query_sql_raw" => ToolAuthz::Needs(DatabaseRead),
        "create_object_schema" | "create_object_instance" | "bulk_create_instances" => {
            ToolAuthz::Needs(DatabaseCreate)
        }
        "update_object_schema" | "update_object_instance" | "bulk_update_instances" => {
            ToolAuthz::Needs(DatabaseUpdate)
        }
        // `execute_sql` can run arbitrary DML — classified at the most privileged database tier.
        "delete_object_schema" | "bulk_delete_instances" | "execute_sql" => {
            ToolAuthz::Needs(DatabaseDelete)
        }

        // ── Triggers ────────────────────────────────────────────────────────────────────
        "list_triggers" | "get_trigger" => ToolAuthz::Needs(TriggerRead),
        "create_trigger" => ToolAuthz::Needs(TriggerCreate),
        "update_trigger" => ToolAuthz::Needs(TriggerUpdate),
        "delete_trigger" => ToolAuthz::Needs(TriggerDelete),

        // ── Connections ─────────────────────────────────────────────────────────────────
        "list_connections"
        | "describe_connection"
        | "resolve_connection_resource"
        | "get_integration"
        | "list_integrations" => ToolAuthz::Needs(ConnectionRead),
        // Starting an OAuth authorization mutates connection state (mirrors the REST
        // `connection:update`-gated authorize route).
        "start_connection_authorization" => ToolAuthz::Needs(ConnectionUpdate),

        _ => ToolAuthz::Unknown,
    }
}

/// Whether a credential scoped to `scope` may use `tool`.
///
/// `Metadata`/`Unknown` are allowed here (the route gate stays authoritative for `Unknown`);
/// a classified tool is checked against the scope via [`ApiKeyScope::permits`]. The method is
/// irrelevant for a mapped permission (`POST` is a representative placeholder), matching the
/// route gate's "key on the permission, not the verb" rule.
pub(crate) fn scope_allows_tool(scope: ApiKeyScope, tool: &str) -> bool {
    match tool_authz(tool) {
        ToolAuthz::Metadata | ToolAuthz::Unknown => true,
        ToolAuthz::Needs(permission) => scope.permits(Some(permission), &Method::POST),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool the MCP server exposes must be classified — a new tool added without a row
    /// here fails this test (it would otherwise ship as `Unknown`, silently relying on the
    /// route gate alone and showing up for every scoped session). Keep this list in sync with
    /// the `#[tool]` functions in `mcp::server`.
    const ALL_TOOLS: &[&str] = &[
        "add_agent_step",
        "add_step",
        "apply_graph_mutations",
        "bulk_create_instances",
        "bulk_delete_instances",
        "bulk_update_instances",
        "compile_workflow",
        "connect_steps",
        "create_object_instance",
        "create_object_schema",
        "create_trigger",
        "create_workflow",
        "delete_object_schema",
        "delete_trigger",
        "delete_workflow",
        "deploy_latest",
        "deploy_workflow",
        "describe_connection",
        "diff_workflow_versions",
        "disconnect_steps",
        "execute_sql",
        "execute_workflow",
        "execute_workflow_sync",
        "execute_workflow_wait",
        "find_references",
        "get_agent",
        "get_capability",
        "get_execution",
        "get_input_schema",
        "get_integration",
        "get_object_schema",
        "get_output_schema",
        "get_signal_schema",
        "get_state_schema",
        "get_step",
        "get_step_edges",
        "get_step_events",
        "get_step_mappings",
        "get_step_summaries",
        "get_step_type_schema",
        "get_trigger",
        "get_workflow",
        "get_workflow_authoring_schema",
        "get_workflow_metadata",
        "get_workflow_slice",
        "inspect_step",
        "list_agents",
        "list_connections",
        "list_edges",
        "list_executions",
        "list_integrations",
        "list_object_instances",
        "list_object_schemas",
        "list_pending_signals",
        "list_references",
        "list_steps",
        "list_step_types",
        "list_triggers",
        "list_unmapped_inputs",
        "list_variables",
        "list_workflow_folders",
        "list_workflows",
        "move_workflow",
        "patch_step",
        "pause_execution",
        "preflight_compile",
        "publish_workflow_agent",
        "query_aggregate",
        "query_object_instances",
        "query_sql",
        "query_sql_one",
        "query_sql_raw",
        "remove_input_schema_field",
        "remove_mapping",
        "remove_step",
        "remove_variable",
        "resolve_connection_resource",
        "resume_execution",
        "set_current_version",
        "set_entry_point",
        "set_input_schema",
        "set_input_schema_field",
        "set_mapping",
        "set_output_schema",
        "set_state_schema",
        "set_variable",
        "set_workflow_metadata",
        "set_workflow_slug",
        "start_connection_authorization",
        "stop_execution",
        "submit_action_response",
        "submit_signal_response",
        "summarize_workflow",
        "test_capability",
        "trace_reference",
        "update_object_instance",
        "update_object_schema",
        "update_step",
        "update_trigger",
        "update_workflow",
        "validate_graph",
        "validate_mappings",
        "why_execution_failed",
    ];

    #[test]
    fn every_tool_is_classified() {
        let unknown: Vec<&str> = ALL_TOOLS
            .iter()
            .copied()
            .filter(|t| tool_authz(t) == ToolAuthz::Unknown)
            .collect();
        assert!(unknown.is_empty(), "unclassified MCP tools: {unknown:?}");
    }

    #[test]
    fn full_scope_allows_every_tool() {
        for tool in ALL_TOOLS {
            assert!(scope_allows_tool(ApiKeyScope::Full, tool), "{tool}");
        }
    }

    #[test]
    fn read_only_allows_reads_and_metadata_but_not_writes() {
        assert!(scope_allows_tool(ApiKeyScope::ReadOnly, "list_workflows"));
        assert!(scope_allows_tool(ApiKeyScope::ReadOnly, "query_sql"));
        assert!(scope_allows_tool(ApiKeyScope::ReadOnly, "list_step_types")); // metadata
        assert!(!scope_allows_tool(ApiKeyScope::ReadOnly, "create_workflow"));
        assert!(!scope_allows_tool(ApiKeyScope::ReadOnly, "execute_sql"));
        assert!(!scope_allows_tool(
            ApiKeyScope::ReadOnly,
            "execute_workflow"
        ));
    }

    #[test]
    fn restricted_scope_allows_exactly_its_tools() {
        use crate::authz::PermissionSet;
        // A key scoped to "run workflows": read + execute, nothing else.
        let scope = ApiKeyScope::Restricted(PermissionSet::from_permissions([
            Permission::WorkflowRead,
            Permission::WorkflowExecute,
        ]));
        assert!(scope_allows_tool(scope, "list_workflows"));
        assert!(scope_allows_tool(scope, "execute_workflow"));
        assert!(scope_allows_tool(scope, "list_agents")); // metadata always allowed
        assert!(!scope_allows_tool(scope, "create_workflow"));
        assert!(!scope_allows_tool(scope, "query_sql")); // database:read not in set
        assert!(!scope_allows_tool(scope, "list_connections"));
    }
}
