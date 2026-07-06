//! Atomic batch edits over [`ReportDefinition`].
//!
//! [`ReportEditOp`] is the canonical mutation primitive — REST and MCP
//! both build a `Vec<ReportEditOp>` and call [`apply_edit_ops`] (or POST
//! to `/api/runtime/reports/{id}/edit` which calls it). Phase 6 collapses
//! the five per-block REST handlers + five MCP layout-node walkers into
//! this one path.
//!
//! Atomicity guarantee: [`apply_edit_ops`] clones the input upfront,
//! applies the batch to the clone, and only writes back on success. A
//! failure at any op leaves the caller's definition untouched.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    ReportBlockDefinition, ReportDefinition, ReportGridLayoutItem, ReportGridLayoutNode,
    ReportLayoutNode,
};

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, rename = "beforeId", skip_serializing_if = "Option::is_none")]
    pub before_id: Option<String>,
    #[serde(default, rename = "afterId", skip_serializing_if = "Option::is_none")]
    pub after_id: Option<String>,
}

/// Destination for a layout-node insert / move.
///
/// `parent_node_id` must reference a `Grid` node and the operation
/// targets that grid's `items` array. When `None`, the operation targets
/// the report's mandatory root grid (`definition.layout`) — that's the
/// only valid container outside an explicit `parent_node_id`. The
/// position fields are mutually exclusive — pick one of
/// index / before_id / after_id (or none, in which case the node is
/// appended at the end).
///
/// Phase 9 collapse: previous versions carried a `columnId` field for
/// the legacy `columns` layout type. With grid-only containers there is
/// no second-level indirection — items live directly on a Grid.
/// Phase 10 collapse: `parent_node_id == None` used to mean "root Vec
/// of layout nodes"; the root is now a single grid, so `None` resolves
/// to that root grid's items.
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutTarget {
    #[serde(
        default,
        rename = "parentNodeId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, rename = "beforeId", skip_serializing_if = "Option::is_none")]
    pub before_id: Option<String>,
    #[serde(default, rename = "afterId", skip_serializing_if = "Option::is_none")]
    pub after_id: Option<String>,
    /// Phase 11: explicit cell position inside the target grid. When set,
    /// the inserted/moved item is placed at this column (1-indexed) so
    /// the renderer pins it to a specific grid cell via CSS
    /// `grid-column`/`grid-row`. Both fields must be set together; if
    /// only one is set the other defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<i64>,
    /// Selects which layout tree the op targets. `None` (the default) is the
    /// report's root layout (`definition.layout`); `Some(view_id)` targets
    /// that `definition.views[].layout` tree. `parent_node_id` / positional
    /// anchors then resolve *within* the selected tree.
    #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportEditOp {
    AddBlock {
        block: ReportBlockDefinition,
        #[serde(default)]
        position: BlockPosition,
    },
    ReplaceBlock {
        #[serde(rename = "blockId")]
        block_id: String,
        block: ReportBlockDefinition,
    },
    PatchBlock {
        #[serde(rename = "blockId")]
        block_id: String,
        patch: Value,
    },
    MoveBlock {
        #[serde(rename = "blockId")]
        block_id: String,
        #[serde(default)]
        position: BlockPosition,
    },
    RemoveBlock {
        #[serde(rename = "blockId")]
        block_id: String,
    },
    AddLayoutNode {
        node: ReportLayoutNode,
        #[serde(default)]
        target: LayoutTarget,
    },
    ReplaceLayoutNode {
        #[serde(rename = "nodeId")]
        node_id: String,
        node: ReportLayoutNode,
        /// Layout tree selector; `None` = report root, `Some(id)` = that view.
        #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
        view_id: Option<String>,
    },
    PatchLayoutNode {
        #[serde(rename = "nodeId")]
        node_id: String,
        patch: Value,
        #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
        view_id: Option<String>,
    },
    MoveLayoutNode {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(default)]
        target: LayoutTarget,
    },
    RemoveLayoutNode {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
        view_id: Option<String>,
    },
}

/// Error from [`apply_edit_ops`]. `code` is a stable SCREAMING_SNAKE_CASE
/// identifier; `message` carries a human-readable description with the
/// failing op index prefixed.
#[derive(Debug, Clone)]
pub struct EditOpError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for EditOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EditOpError {}

fn err(code: &'static str, message: impl Into<String>) -> EditOpError {
    EditOpError {
        code,
        message: message.into(),
    }
}

/// Apply a batch of edit ops atomically. Returns `Ok(())` on success;
/// on failure the caller's `definition` is unchanged.
pub fn apply_edit_ops(
    definition: &mut ReportDefinition,
    ops: &[ReportEditOp],
) -> Result<(), EditOpError> {
    let mut working = definition.clone();
    for (index, op) in ops.iter().enumerate() {
        apply_op(&mut working, op).map_err(|e| EditOpError {
            code: e.code,
            message: format!("op {} ({}): {}", index, op_kind(op), e.message),
        })?;
    }
    *definition = working;
    Ok(())
}

/// Top-level keys permitted on each op kind's JSON object. Positional /
/// parent / view fields live under a nested `target` (layout add/move) or
/// `position` (block add/move) object, NOT at the op top level. Sending them
/// at the top level was the silent-drop root cause behind the "appended to
/// root with no error" and "beforeId ignored / append-only" reports:
/// `ReportEditOp` is an internally-tagged enum, so serde cannot enforce
/// `deny_unknown_fields` on it and silently discarded the misplaced keys.
/// [`validate_edit_ops_json`] restores loud rejection with a targeted hint.
///
/// Returns `None` for an unrecognized kind.
fn allowed_top_level_keys(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "add_block" => &["kind", "block", "position"],
        "replace_block" => &["kind", "blockId", "block"],
        "patch_block" => &["kind", "blockId", "patch"],
        "move_block" => &["kind", "blockId", "position"],
        "remove_block" => &["kind", "blockId"],
        "add_layout_node" => &["kind", "node", "target"],
        // `viewId` (Phase: view targeting) is a top-level field on the
        // ops that carry no `target`; on add/move it lives under `target`.
        "replace_layout_node" => &["kind", "nodeId", "node", "viewId"],
        "patch_layout_node" => &["kind", "nodeId", "patch", "viewId"],
        "move_layout_node" => &["kind", "nodeId", "target"],
        "remove_layout_node" => &["kind", "nodeId", "viewId"],
        _ => return None,
    })
}

/// A targeted hint for a stray top-level key, naming the correct nested
/// location or spelling. Returns `None` when no specific guidance applies
/// (the caller falls back to listing the allowed fields).
fn misplaced_key_hint(kind: &str, key: &str) -> Option<String> {
    // The wrapper object where positional / parent fields belong for this kind.
    let wrapper = match kind {
        "add_layout_node" | "move_layout_node" => Some("target"),
        "add_block" | "move_block" => Some("position"),
        _ => None,
    };
    match key {
        "parentNodeId" | "col" | "row" => {
            // These only exist under a layout `target`.
            match kind {
                "add_layout_node" | "move_layout_node" => Some(format!(
                    "'{key}' belongs under `target` for {kind}, not at the op top level"
                )),
                _ => None,
            }
        }
        "beforeId" | "afterId" | "index" => wrapper
            .map(|w| format!("'{key}' belongs under `{w}` for {kind}, not at the op top level")),
        "viewId" => match kind {
            "add_layout_node" | "move_layout_node" => Some(format!(
                "'viewId' belongs under `target` for {kind}, not at the op top level"
            )),
            _ => None,
        },
        "beforeNodeId" => wrapper.map(|w| {
            format!("unknown field 'beforeNodeId' — did you mean 'beforeId' under `{w}`?")
        }),
        "afterNodeId" => wrapper
            .map(|w| format!("unknown field 'afterNodeId' — did you mean 'afterId' under `{w}`?")),
        "parentId" => Some(
            "unknown field 'parentId' — did you mean 'parentNodeId' under `target`?".to_string(),
        ),
        "before" | "after" => {
            wrapper.map(|w| format!("unknown field '{key}' — did you mean '{key}Id' under `{w}`?"))
        }
        _ => None,
    }
}

/// Reject malformed edit-op JSON *before* it is deserialized into the typed
/// [`ReportEditOp`] enum. Because the enum is internally tagged, serde cannot
/// carry `deny_unknown_fields`, so a top-level key that belongs under `target`
/// / `position` (or a misspelling like `beforeNodeId`) would otherwise be
/// silently dropped — the op then no-ops or lands in the wrong place with a
/// success result. This pass runs per op, keyed on `kind`, and errors with a
/// stable `UNKNOWN_OP_FIELD` code plus a targeted hint. Typos *inside* a
/// correctly-nested `target` / `position` are caught separately by
/// `deny_unknown_fields` on [`LayoutTarget`] / [`BlockPosition`] at the typed
/// deserialization step.
pub fn validate_edit_ops_json(raw_ops: &[Value]) -> Result<(), EditOpError> {
    const KNOWN_KINDS: &str = "add_block, replace_block, patch_block, move_block, remove_block, \
         add_layout_node, replace_layout_node, patch_layout_node, move_layout_node, \
         remove_layout_node";
    for (index, op) in raw_ops.iter().enumerate() {
        let Some(obj) = op.as_object() else {
            return Err(err(
                "INVALID_OP",
                format!("op {index}: each edit op must be a JSON object"),
            ));
        };
        let Some(kind) = obj.get("kind").and_then(|v| v.as_str()) else {
            return Err(err(
                "MISSING_OP_KIND",
                format!("op {index}: missing string 'kind' (one of {KNOWN_KINDS})"),
            ));
        };
        let Some(allowed) = allowed_top_level_keys(kind) else {
            return Err(err(
                "UNKNOWN_OP_KIND",
                format!("op {index}: unknown op kind '{kind}' (expected one of {KNOWN_KINDS})"),
            ));
        };
        for key in obj.keys() {
            if allowed.contains(&key.as_str()) {
                continue;
            }
            let hint = misplaced_key_hint(kind, key)
                .unwrap_or_else(|| format!("allowed fields for {kind}: {}", allowed.join(", ")));
            return Err(err(
                "UNKNOWN_OP_FIELD",
                format!("op {index} ({kind}): unknown field '{key}'. {hint}"),
            ));
        }
    }
    Ok(())
}

fn op_kind(op: &ReportEditOp) -> &'static str {
    match op {
        ReportEditOp::AddBlock { .. } => "add_block",
        ReportEditOp::ReplaceBlock { .. } => "replace_block",
        ReportEditOp::PatchBlock { .. } => "patch_block",
        ReportEditOp::MoveBlock { .. } => "move_block",
        ReportEditOp::RemoveBlock { .. } => "remove_block",
        ReportEditOp::AddLayoutNode { .. } => "add_layout_node",
        ReportEditOp::ReplaceLayoutNode { .. } => "replace_layout_node",
        ReportEditOp::PatchLayoutNode { .. } => "patch_layout_node",
        ReportEditOp::MoveLayoutNode { .. } => "move_layout_node",
        ReportEditOp::RemoveLayoutNode { .. } => "remove_layout_node",
    }
}

fn apply_op(def: &mut ReportDefinition, op: &ReportEditOp) -> Result<(), EditOpError> {
    match op {
        ReportEditOp::AddBlock { block, position } => add_block(def, block.clone(), position),
        ReportEditOp::ReplaceBlock { block_id, block } => {
            replace_block(def, block_id, block.clone())
        }
        ReportEditOp::PatchBlock { block_id, patch } => patch_block(def, block_id, patch),
        ReportEditOp::MoveBlock { block_id, position } => move_block(def, block_id, position),
        ReportEditOp::RemoveBlock { block_id } => remove_block(def, block_id),
        ReportEditOp::AddLayoutNode { node, target } => add_layout_node(def, node.clone(), target),
        ReportEditOp::ReplaceLayoutNode {
            node_id,
            node,
            view_id,
        } => replace_layout_node(def, node_id, node.clone(), view_id.as_deref()),
        ReportEditOp::PatchLayoutNode {
            node_id,
            patch,
            view_id,
        } => patch_layout_node(def, node_id, patch, view_id.as_deref()),
        ReportEditOp::MoveLayoutNode { node_id, target } => move_layout_node(def, node_id, target),
        ReportEditOp::RemoveLayoutNode { node_id, view_id } => {
            remove_layout_node(def, node_id, view_id.as_deref())
        }
    }
}

// ============================================================================
// Block ops
// ============================================================================

fn add_block(
    def: &mut ReportDefinition,
    block: ReportBlockDefinition,
    position: &BlockPosition,
) -> Result<(), EditOpError> {
    if def.blocks.iter().any(|b| b.id == block.id) {
        return Err(err(
            "DUPLICATE_BLOCK_ID",
            format!("Block '{}' already exists", block.id),
        ));
    }
    let index = resolve_block_index(&def.blocks, position)?;
    def.blocks.insert(index, block);
    Ok(())
}

fn replace_block(
    def: &mut ReportDefinition,
    block_id: &str,
    block: ReportBlockDefinition,
) -> Result<(), EditOpError> {
    if block.id != block_id {
        return Err(err(
            "BLOCK_ID_IMMUTABLE",
            format!(
                "Replacement block id '{}' does not match target '{}'",
                block.id, block_id
            ),
        ));
    }
    let index = find_block_index(def, block_id)?;
    def.blocks[index] = block;
    Ok(())
}

fn patch_block(
    def: &mut ReportDefinition,
    block_id: &str,
    patch: &Value,
) -> Result<(), EditOpError> {
    if !patch.is_object() {
        return Err(err(
            "INVALID_PATCH",
            "Report block patch must be a JSON object",
        ));
    }
    if patch.get("id").is_some() {
        return Err(err(
            "BLOCK_ID_IMMUTABLE",
            "Report block id cannot be changed with patch_block",
        ));
    }
    let index = find_block_index(def, block_id)?;
    let mut block_value = serde_json::to_value(&def.blocks[index])
        .map_err(|e| err("INVALID_PATCH", e.to_string()))?;
    apply_json_merge_patch(&mut block_value, patch);
    let patched: ReportBlockDefinition = serde_json::from_value(block_value)
        .map_err(|e| err("INVALID_PATCH", format!("Invalid block patch: {}", e)))?;
    if patched.id != block_id {
        return Err(err(
            "BLOCK_ID_IMMUTABLE",
            "Report block id cannot be changed with patch_block",
        ));
    }
    def.blocks[index] = patched;
    Ok(())
}

fn move_block(
    def: &mut ReportDefinition,
    block_id: &str,
    position: &BlockPosition,
) -> Result<(), EditOpError> {
    let current = find_block_index(def, block_id)?;
    let block = def.blocks.remove(current);
    let new_index = match resolve_block_index(&def.blocks, position) {
        Ok(i) => i,
        Err(e) => {
            // Roll back the remove before bubbling up.
            def.blocks.insert(current, block);
            return Err(e);
        }
    };
    def.blocks.insert(new_index, block);
    Ok(())
}

fn remove_block(def: &mut ReportDefinition, block_id: &str) -> Result<(), EditOpError> {
    let index = find_block_index(def, block_id)?;
    def.blocks.remove(index);
    Ok(())
}

fn find_block_index(def: &ReportDefinition, block_id: &str) -> Result<usize, EditOpError> {
    def.blocks
        .iter()
        .position(|b| b.id == block_id)
        .ok_or_else(|| {
            err(
                "BLOCK_NOT_FOUND",
                format!("Report block '{}' not found", block_id),
            )
        })
}

fn resolve_block_index(
    blocks: &[ReportBlockDefinition],
    position: &BlockPosition,
) -> Result<usize, EditOpError> {
    match (
        position.index,
        position.before_id.as_deref(),
        position.after_id.as_deref(),
    ) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => Err(err(
            "INVALID_POSITION",
            "Position fields index/beforeId/afterId are mutually exclusive",
        )),
        (Some(i), _, _) => Ok(i.min(blocks.len())),
        (_, Some(before), _) => blocks.iter().position(|b| b.id == before).ok_or_else(|| {
            err(
                "BLOCK_NOT_FOUND",
                format!("Position anchor block '{}' not found", before),
            )
        }),
        (_, _, Some(after)) => blocks
            .iter()
            .position(|b| b.id == after)
            .map(|i| i + 1)
            .ok_or_else(|| {
                err(
                    "BLOCK_NOT_FOUND",
                    format!("Position anchor block '{}' not found", after),
                )
            }),
        (None, None, None) => Ok(blocks.len()),
    }
}

// ============================================================================
// Layout ops
// ============================================================================

/// Resolve which layout tree an op targets. `None` (the default) is the
/// report's root layout (`definition.layout`); `Some(view_id)` is that
/// `definition.views[].layout`. Errors with `VIEW_NOT_FOUND` for an unknown
/// view id. All node resolution/insertion then happens within the returned
/// tree, so `parent_node_id`/anchors are scoped to it.
fn target_layout_mut<'a>(
    def: &'a mut ReportDefinition,
    view_id: Option<&str>,
) -> Result<&'a mut ReportGridLayoutNode, EditOpError> {
    match view_id {
        None => Ok(&mut def.layout),
        Some(id) => def
            .views
            .iter_mut()
            .find(|v| v.id == id)
            .map(|v| &mut v.layout)
            .ok_or_else(|| err("VIEW_NOT_FOUND", format!("Report view '{id}' not found"))),
    }
}

/// `true` if a layout node with `node_id` exists in the root layout or any
/// view layout. Used for a GLOBAL uniqueness check on `AddLayoutNode` so a
/// node id can never collide across the root and view trees (blocks are keyed
/// by id and future lookups may not be view-scoped). Backward-compatible:
/// existing reports carry no cross-tree duplicate ids.
fn layout_node_id_exists_anywhere(def: &ReportDefinition, node_id: &str) -> bool {
    if def.layout.id == node_id || layout_node_exists_in_grid(&def.layout, node_id) {
        return true;
    }
    def.views
        .iter()
        .any(|v| v.layout.id == node_id || layout_node_exists_in_grid(&v.layout, node_id))
}

fn add_layout_node(
    def: &mut ReportDefinition,
    node: ReportLayoutNode,
    target: &LayoutTarget,
) -> Result<(), EditOpError> {
    let node_id = layout_node_id(&node).to_string();
    if layout_node_id_exists_anywhere(def, &node_id) {
        return Err(err(
            "DUPLICATE_LAYOUT_NODE_ID",
            format!("Layout node '{}' already exists", node_id),
        ));
    }
    let root = target_layout_mut(def, target.view_id.as_deref())?;
    insert_into_target(root, target, node)
}

fn replace_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    node: ReportLayoutNode,
    view_id: Option<&str>,
) -> Result<(), EditOpError> {
    if layout_node_id(&node) != node_id {
        return Err(err(
            "LAYOUT_NODE_ID_IMMUTABLE",
            format!(
                "Replacement layout node id '{}' does not match target '{}'",
                layout_node_id(&node),
                node_id
            ),
        ));
    }
    let root = target_layout_mut(def, view_id)?;
    if node_id == root.id {
        // Replacing the tree root: must remain a grid (it's the layout type).
        let ReportLayoutNode::Grid(grid) = node else {
            return Err(err(
                "ROOT_LAYOUT_MUST_BE_GRID",
                "The root layout node must be a grid; cannot replace with a block",
            ));
        };
        *root = grid;
        return Ok(());
    }
    if !replace_in_grid(root, node_id, node) {
        return Err(layout_not_found(node_id));
    }
    Ok(())
}

fn patch_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    patch: &Value,
    view_id: Option<&str>,
) -> Result<(), EditOpError> {
    if !patch.is_object() {
        return Err(err(
            "INVALID_PATCH",
            "Layout node patch must be a JSON object",
        ));
    }
    if patch.get("id").is_some() {
        return Err(err(
            "LAYOUT_NODE_ID_IMMUTABLE",
            "Layout node id cannot be changed with patch_layout_node",
        ));
    }
    let root = target_layout_mut(def, view_id)?;
    if patch.get("type").is_some() && node_id == root.id {
        return Err(err(
            "ROOT_LAYOUT_MUST_BE_GRID",
            "The root layout node's type cannot be changed; root must remain a grid",
        ));
    }
    // Special case: patching the tree root grid in-place.
    if node_id == root.id {
        let mut node_value =
            serde_json::to_value(&*root).map_err(|e| err("INVALID_PATCH", e.to_string()))?;
        // Treat the root grid's wire form as `{type: "grid", ...}` for
        // patch purposes so callers can write the same patches they'd
        // use against any other grid. We add `type` only if absent so
        // we don't conflict with the explicit-rejection above.
        if let Value::Object(map) = &mut node_value {
            map.entry("type".to_string())
                .or_insert(Value::String("grid".to_string()));
        }
        apply_json_merge_patch(&mut node_value, patch);
        // Drop the synthetic `type` field if it survived the patch — the
        // serialized `ReportGridLayoutNode` doesn't carry one.
        if let Value::Object(map) = &mut node_value {
            map.remove("type");
        }
        let patched: ReportGridLayoutNode = serde_json::from_value(node_value)
            .map_err(|e| err("INVALID_PATCH", format!("Invalid root grid patch: {}", e)))?;
        if patched.id != node_id {
            return Err(err(
                "LAYOUT_NODE_ID_IMMUTABLE",
                "Layout node id cannot be changed with patch_layout_node",
            ));
        }
        *root = patched;
        return Ok(());
    }
    let Some(node) = find_in_grid_mut(root, node_id) else {
        return Err(layout_not_found(node_id));
    };
    let mut node_value =
        serde_json::to_value(&*node).map_err(|e| err("INVALID_PATCH", e.to_string()))?;
    apply_json_merge_patch(&mut node_value, patch);
    let patched: ReportLayoutNode = serde_json::from_value(node_value)
        .map_err(|e| err("INVALID_PATCH", format!("Invalid layout patch: {}", e)))?;
    if layout_node_id(&patched) != node_id {
        return Err(err(
            "LAYOUT_NODE_ID_IMMUTABLE",
            "Layout node id cannot be changed with patch_layout_node",
        ));
    }
    *node = patched;
    Ok(())
}

fn move_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    target: &LayoutTarget,
) -> Result<(), EditOpError> {
    let view_id = target.view_id.clone();
    // Reject moving a tree's own root grid.
    {
        let root = target_layout_mut(def, view_id.as_deref())?;
        if node_id == root.id {
            return Err(err(
                "CANNOT_MOVE_ROOT_GRID",
                "The root layout grid cannot be moved",
            ));
        }
    }
    // Remove from the target tree. Source and destination share `target.viewId`,
    // so this is an intra-tree reposition.
    let removed = {
        let root = target_layout_mut(def, view_id.as_deref())?;
        remove_from_grid(root, node_id)
    };
    let node = match removed {
        Some(node) => node,
        None => {
            // Cross-tree move (root <-> view, or between views) is unsupported:
            // if the node lives in a *different* tree, say so explicitly rather
            // than returning an opaque not-found.
            if layout_node_id_exists_anywhere(def, node_id) {
                return Err(err(
                    "CROSS_TREE_MOVE_UNSUPPORTED",
                    format!(
                        "Layout node '{node_id}' is in a different layout tree than the move target (viewId={view_id:?}); cross-tree moves are unsupported — use remove_layout_node in the source tree then add_layout_node in the destination tree"
                    ),
                ));
            }
            let root = target_layout_mut(def, view_id.as_deref())?;
            return Err(layout_not_found_with_item_hint(root, node_id));
        }
    };
    let root = target_layout_mut(def, view_id.as_deref())?;
    insert_into_target(root, target, node)
}

fn remove_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    view_id: Option<&str>,
) -> Result<(), EditOpError> {
    let root = target_layout_mut(def, view_id)?;
    if node_id == root.id {
        return Err(err(
            "CANNOT_REMOVE_ROOT_GRID",
            "The root layout grid cannot be removed",
        ));
    }
    if remove_from_grid(root, node_id).is_none() {
        return Err(layout_not_found_with_item_hint(root, node_id));
    }
    Ok(())
}

// ----- Layout tree walking -----

fn layout_node_id(node: &ReportLayoutNode) -> &str {
    match node {
        ReportLayoutNode::Block(n) => &n.id,
        ReportLayoutNode::Grid(n) => &n.id,
    }
}

fn layout_not_found(node_id: &str) -> EditOpError {
    err(
        "LAYOUT_NODE_NOT_FOUND",
        format!("Layout node '{}' not found", node_id),
    )
}

/// Like [`layout_not_found`], but if `node_id` matches a grid *item-wrapper*
/// id (the `{ id, child }` envelope) rather than a child layout-node id,
/// append a hint naming the child node id to pass instead. Layout ops address
/// nodes by their child node id, not the item-wrapper id — an easy wrong guess
/// because `get_report` surfaces both.
fn layout_not_found_with_item_hint(grid: &ReportGridLayoutNode, node_id: &str) -> EditOpError {
    if let Some(child_id) = find_item_wrapper_child_id(grid, node_id) {
        return err(
            "LAYOUT_NODE_NOT_FOUND",
            format!(
                "Layout node '{node_id}' not found — '{node_id}' is an item-wrapper id; pass the child node id '{child_id}' instead"
            ),
        );
    }
    layout_not_found(node_id)
}

/// Find the child node id of the grid item whose *wrapper* id equals
/// `item_id`, searching nested grids. Mirrors [`remove_from_grid`]'s recursion
/// but matches on `item.id`.
fn find_item_wrapper_child_id<'a>(
    grid: &'a ReportGridLayoutNode,
    item_id: &str,
) -> Option<&'a str> {
    for item in &grid.items {
        if item.id == item_id {
            return Some(layout_node_id(&item.child));
        }
        if let ReportLayoutNode::Grid(nested) = item.child.as_ref()
            && let Some(found) = find_item_wrapper_child_id(nested, item_id)
        {
            return Some(found);
        }
    }
    None
}

/// `true` if a layout node with `node_id` is anywhere under `grid`'s
/// items (excluding `grid` itself). Used to reject `AddLayoutNode` of a
/// duplicate id.
fn layout_node_exists_in_grid(grid: &ReportGridLayoutNode, node_id: &str) -> bool {
    for item in &grid.items {
        if layout_node_id(&item.child) == node_id {
            return true;
        }
        if let ReportLayoutNode::Grid(nested) = item.child.as_ref()
            && (nested.id == node_id || layout_node_exists_in_grid(nested, node_id))
        {
            return true;
        }
    }
    false
}

/// Walk the root grid's tree looking for a non-root layout node with
/// `node_id`. The root grid itself is intentionally not reachable here
/// — callers handle the root-grid case before calling.
fn find_in_grid_mut<'a>(
    grid: &'a mut ReportGridLayoutNode,
    node_id: &str,
) -> Option<&'a mut ReportLayoutNode> {
    for item in grid.items.iter_mut() {
        if layout_node_id(&item.child) == node_id {
            return Some(item.child.as_mut());
        }
        if let ReportLayoutNode::Grid(nested) = item.child.as_mut()
            && let Some(found) = find_in_grid_mut(nested, node_id)
        {
            return Some(found);
        }
    }
    None
}

fn replace_in_grid(
    grid: &mut ReportGridLayoutNode,
    node_id: &str,
    replacement: ReportLayoutNode,
) -> bool {
    for item in grid.items.iter_mut() {
        if layout_node_id(&item.child) == node_id {
            *item.child.as_mut() = replacement;
            return true;
        }
        if let ReportLayoutNode::Grid(nested) = item.child.as_mut()
            && replace_in_grid(nested, node_id, replacement.clone())
        {
            return true;
        }
    }
    false
}

fn remove_from_grid(grid: &mut ReportGridLayoutNode, node_id: &str) -> Option<ReportLayoutNode> {
    if let Some(index) = grid
        .items
        .iter()
        .position(|item| layout_node_id(&item.child) == node_id)
    {
        let removed_item = grid.items.remove(index);
        return Some(*removed_item.child);
    }
    for item in grid.items.iter_mut() {
        if let ReportLayoutNode::Grid(nested) = item.child.as_mut()
            && let Some(removed) = remove_from_grid(nested, node_id)
        {
            return Some(removed);
        }
    }
    None
}

/// Resolve the destination container for [`AddLayoutNode`]/[`MoveLayoutNode`]
/// and insert. With the root being a single grid, `parent_node_id` either
/// references that root grid (or `None` → root grid implicit) or an
/// inner nested grid.
fn insert_into_target(
    root: &mut ReportGridLayoutNode,
    target: &LayoutTarget,
    node: ReportLayoutNode,
) -> Result<(), EditOpError> {
    let target_grid = resolve_target_grid(root, target.parent_node_id.as_deref())?;
    let index = resolve_grid_item_index(&target_grid.items, target)?;
    let item = wrap_in_grid_item(node, target.col, target.row);
    target_grid.items.insert(index, item);
    Ok(())
}

/// Pick the grid that an `AddLayoutNode`/`MoveLayoutNode` target points
/// at. `None` resolves to the root grid; a `Some(parent_id)` resolves
/// to that node in the tree and rejects non-grid targets.
fn resolve_target_grid<'a>(
    root: &'a mut ReportGridLayoutNode,
    parent_node_id: Option<&str>,
) -> Result<&'a mut ReportGridLayoutNode, EditOpError> {
    let Some(parent_id) = parent_node_id else {
        return Ok(root);
    };
    if parent_id == root.id {
        return Ok(root);
    }
    let Some(parent) = find_in_grid_mut(root, parent_id) else {
        return Err(layout_not_found(parent_id));
    };
    let ReportLayoutNode::Grid(grid) = parent else {
        return Err(err(
            "INVALID_LAYOUT_TARGET",
            "parentNodeId must reference a grid layout node",
        ));
    };
    Ok(grid)
}

fn wrap_in_grid_item(
    node: ReportLayoutNode,
    col: Option<i64>,
    row: Option<i64>,
) -> ReportGridLayoutItem {
    let id = format!("item_{}", layout_node_id(&node));
    ReportGridLayoutItem {
        id,
        col,
        row,
        col_span: None,
        row_span: None,
        child: Box::new(node),
    }
}

/// Resolve a position into a grid's `items` array. `before_id` /
/// `after_id` reference the child layout-node id (not the grid item id),
/// since callers identify nodes, not item wrappers.
fn resolve_grid_item_index(
    items: &[ReportGridLayoutItem],
    target: &LayoutTarget,
) -> Result<usize, EditOpError> {
    match (
        target.index,
        target.before_id.as_deref(),
        target.after_id.as_deref(),
    ) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => Err(err(
            "INVALID_POSITION",
            "Position fields index/beforeId/afterId are mutually exclusive",
        )),
        (Some(i), _, _) => Ok(i.min(items.len())),
        (_, Some(before), _) => items
            .iter()
            .position(|item| layout_node_id(&item.child) == before)
            .ok_or_else(|| {
                err(
                    "LAYOUT_ANCHOR_NOT_FOUND",
                    format!("Position anchor layout node '{}' not found", before),
                )
            }),
        (_, _, Some(after)) => items
            .iter()
            .position(|item| layout_node_id(&item.child) == after)
            .map(|i| i + 1)
            .ok_or_else(|| {
                err(
                    "LAYOUT_ANCHOR_NOT_FOUND",
                    format!("Position anchor layout node '{}' not found", after),
                )
            }),
        (None, None, None) => Ok(items.len()),
    }
}

// ============================================================================
// JSON merge patch (RFC 7386)
// ============================================================================

/// Apply an RFC 7386 JSON merge patch in place.
pub fn apply_json_merge_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, patch_value) in patch {
                if patch_value.is_null() {
                    target.remove(key);
                } else {
                    apply_json_merge_patch(
                        target.entry(key.clone()).or_insert(Value::Null),
                        patch_value,
                    );
                }
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_def() -> ReportDefinition {
        serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": []
        }))
        .unwrap()
    }

    fn block(id: &str) -> ReportBlockDefinition {
        serde_json::from_value(json!({
            "id": id,
            "type": "markdown",
            "markdown": { "content": "x" }
        }))
        .unwrap()
    }

    #[test]
    fn add_block_appends_when_no_position() {
        let mut def = empty_def();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddBlock {
                block: block("a"),
                position: BlockPosition::default(),
            }],
        )
        .unwrap();
        assert_eq!(def.blocks.len(), 1);
        assert_eq!(def.blocks[0].id, "a");
    }

    #[test]
    fn add_block_rejects_duplicate_id() {
        let mut def = empty_def();
        def.blocks.push(block("a"));
        let err = apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddBlock {
                block: block("a"),
                position: BlockPosition::default(),
            }],
        )
        .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_BLOCK_ID");
        // atomic: definition untouched on failure
        assert_eq!(def.blocks.len(), 1);
    }

    #[test]
    fn patch_block_rejects_id_change() {
        let mut def = empty_def();
        def.blocks.push(block("a"));
        let err = apply_edit_ops(
            &mut def,
            &[ReportEditOp::PatchBlock {
                block_id: "a".to_string(),
                patch: json!({ "id": "b" }),
            }],
        )
        .unwrap_err();
        assert_eq!(err.code, "BLOCK_ID_IMMUTABLE");
    }

    #[test]
    fn move_block_by_after_id() {
        let mut def = empty_def();
        def.blocks.push(block("a"));
        def.blocks.push(block("b"));
        def.blocks.push(block("c"));
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::MoveBlock {
                block_id: "a".to_string(),
                position: BlockPosition {
                    after_id: Some("b".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(
            def.blocks.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
    }

    #[test]
    fn batch_rolls_back_on_failure() {
        let mut def = empty_def();
        def.blocks.push(block("a"));
        let err = apply_edit_ops(
            &mut def,
            &[
                ReportEditOp::AddBlock {
                    block: block("b"),
                    position: BlockPosition::default(),
                },
                // This duplicates 'a' and must fail; the AddBlock above must
                // not be applied to the input.
                ReportEditOp::AddBlock {
                    block: block("a"),
                    position: BlockPosition::default(),
                },
            ],
        )
        .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_BLOCK_ID");
        // Atomicity: only the original 'a' remains.
        assert_eq!(def.blocks.len(), 1);
        assert_eq!(def.blocks[0].id, "a");
    }

    #[test]
    fn add_layout_node_to_root_grid_via_parent_id() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {"id": "root", "items": []}
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(json!({
                    "type": "block",
                    "id": "ln1",
                    "blockId": "b1"
                }))
                .unwrap(),
                target: LayoutTarget {
                    parent_node_id: Some("root".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(def.layout.items.len(), 1);
        assert_eq!(layout_node_id(&def.layout.items[0].child), "ln1");
    }

    #[test]
    fn add_layout_node_with_none_target_appends_to_root_grid() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {"id": "root", "items": []}
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(json!({
                    "type": "block",
                    "id": "ln1",
                    "blockId": "b1"
                }))
                .unwrap(),
                target: LayoutTarget::default(),
            }],
        )
        .unwrap();
        assert_eq!(def.layout.items.len(), 1);
        assert_eq!(layout_node_id(&def.layout.items[0].child), "ln1");
    }

    #[test]
    fn remove_layout_node_rejects_root_grid() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [],
            "layout": {"id": "root", "items": []}
        }))
        .unwrap();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "root".to_string(),
                view_id: None,
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "CANNOT_REMOVE_ROOT_GRID");
    }

    #[test]
    fn replace_layout_node_rejects_replacing_root_with_block() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {"id": "root", "items": []}
        }))
        .unwrap();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::ReplaceLayoutNode {
                node_id: "root".to_string(),
                view_id: None,
                node: serde_json::from_value(json!({
                    "type": "block",
                    "id": "root",
                    "blockId": "b1"
                }))
                .unwrap(),
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "ROOT_LAYOUT_MUST_BE_GRID");
    }

    #[test]
    fn patch_layout_node_can_update_root_grid_columns() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [],
            "layout": {"id": "root", "items": [], "columns": 1}
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::PatchLayoutNode {
                node_id: "root".to_string(),
                patch: json!({"columns": 3, "title": "Dashboard"}),
                view_id: None,
            }],
        )
        .unwrap();
        assert_eq!(def.layout.columns, Some(3));
        assert_eq!(def.layout.title.as_deref(), Some("Dashboard"));
    }

    #[test]
    fn batched_ops_equivalent_to_sequential_application() {
        // Apply [add b1, add b2, patch b1 title, remove b2] both ways and
        // confirm the result is identical. This proves apply_edit_ops
        // composes sequentially within one call the same way it would
        // across separate calls.
        let ops = vec![
            ReportEditOp::AddBlock {
                block: block("b1"),
                position: BlockPosition::default(),
            },
            ReportEditOp::AddBlock {
                block: block("b2"),
                position: BlockPosition::default(),
            },
            ReportEditOp::PatchBlock {
                block_id: "b1".to_string(),
                patch: json!({ "title": "First" }),
            },
            ReportEditOp::RemoveBlock {
                block_id: "b2".to_string(),
            },
        ];

        let mut batched = empty_def();
        apply_edit_ops(&mut batched, &ops).unwrap();

        let mut sequential = empty_def();
        for op in &ops {
            apply_edit_ops(&mut sequential, std::slice::from_ref(op)).unwrap();
        }

        assert_eq!(
            serde_json::to_value(&batched).unwrap(),
            serde_json::to_value(&sequential).unwrap()
        );
    }

    #[test]
    fn remove_layout_node_finds_nested_match() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {
                "id": "root",
                "items": [{
                    "id": "item_ln1",
                    "child": {"type": "block", "id": "ln1", "blockId": "b1"}
                }]
            }
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "ln1".to_string(),
                view_id: None,
            }],
        )
        .unwrap();
        assert!(def.layout.items.is_empty());
    }

    #[test]
    fn add_layout_node_to_nested_grid() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [
                {"id": "b1", "type": "markdown", "markdown": {"content": "x"}},
                {"id": "b2", "type": "markdown", "markdown": {"content": "y"}}
            ],
            "layout": {
                "id": "outer",
                "columns": 2,
                "items": [{
                    "id": "item_inner",
                    "child": {"type": "grid", "id": "inner", "columns": 1, "items": []}
                }]
            }
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(json!({
                    "type": "block", "id": "ln1", "blockId": "b2"
                }))
                .unwrap(),
                target: LayoutTarget {
                    parent_node_id: Some("inner".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        let inner = match def.layout.items[0].child.as_ref() {
            ReportLayoutNode::Grid(g) => g,
            _ => panic!("expected nested Grid"),
        };
        assert_eq!(inner.items.len(), 1);
        assert_eq!(layout_node_id(&inner.items[0].child), "ln1");
    }

    #[test]
    fn add_layout_node_with_explicit_col_row() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {"id": "root", "columns": 3, "items": []}
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(json!({
                    "type": "block", "id": "ln1", "blockId": "b1"
                }))
                .unwrap(),
                target: LayoutTarget {
                    parent_node_id: Some("root".to_string()),
                    col: Some(2),
                    row: Some(3),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(def.layout.items.len(), 1);
        assert_eq!(def.layout.items[0].col, Some(2));
        assert_eq!(def.layout.items[0].row, Some(3));
    }

    #[test]
    fn move_layout_node_to_explicit_cell() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [
                {"id": "b1", "type": "markdown", "markdown": {"content": "x"}},
                {"id": "b2", "type": "markdown", "markdown": {"content": "y"}}
            ],
            "layout": {
                "id": "root",
                "columns": 3,
                "items": [
                    {"id": "i1", "child": {"type": "block", "id": "n1", "blockId": "b1"}},
                    {"id": "i2", "child": {"type": "block", "id": "n2", "blockId": "b2"}}
                ]
            }
        }))
        .unwrap();
        // Move n1 to cell (col=3, row=2). The item wrapper gets a new
        // synthetic id from wrap_in_grid_item; we only need to assert
        // the col/row landed.
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::MoveLayoutNode {
                node_id: "n1".to_string(),
                target: LayoutTarget {
                    parent_node_id: Some("root".to_string()),
                    col: Some(3),
                    row: Some(2),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        let moved = def
            .layout
            .items
            .iter()
            .find(|item| layout_node_id(&item.child) == "n1")
            .expect("n1 still in layout after move");
        assert_eq!(moved.col, Some(3));
        assert_eq!(moved.row, Some(2));
    }

    #[test]
    fn default_root_grid_used_when_layout_omitted() {
        let def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": []
        }))
        .unwrap();
        // The default root grid should be present even when callers omit
        // the field — Phase 10 made `layout` mandatory in the type
        // system, but `#[serde(default = default_root_grid)]` keeps
        // legacy/minimal JSON payloads usable.
        assert!(def.layout.items.is_empty());
        assert_eq!(def.layout.columns, Some(1));
    }

    // ----- strictness: validate_edit_ops_json -----

    #[test]
    fn validate_edit_ops_accepts_well_formed_ops() {
        let ops = json!([
            {"kind": "add_layout_node", "node": {"type": "block", "id": "n", "blockId": "b"},
             "target": {"parentNodeId": "root", "beforeId": "x"}},
            {"kind": "replace_block", "blockId": "b", "block": {"id": "b", "type": "markdown", "markdown": {"content": "y"}}},
            {"kind": "remove_layout_node", "nodeId": "n", "viewId": "detail"}
        ]);
        validate_edit_ops_json(ops.as_array().unwrap()).unwrap();
    }

    #[test]
    fn validate_edit_ops_rejects_top_level_parent_node_id_with_hint() {
        // The literal reported bug: parentNodeId at the op top level (belongs
        // under `target`) was silently dropped -> appended to root, Ok(()).
        let ops = json!([
            {"kind": "add_layout_node", "parentNodeId": "detail_root",
             "node": {"type": "grid", "id": "g", "items": []}}
        ]);
        let e = validate_edit_ops_json(ops.as_array().unwrap()).unwrap_err();
        assert_eq!(e.code, "UNKNOWN_OP_FIELD");
        assert!(e.message.contains("parentNodeId"), "msg: {}", e.message);
        assert!(e.message.contains("`target`"), "msg: {}", e.message);
    }

    #[test]
    fn validate_edit_ops_hints_before_node_id_spelling() {
        let ops = json!([
            {"kind": "add_layout_node", "node": {"type": "block", "id": "n", "blockId": "b"},
             "beforeNodeId": "x"}
        ]);
        let e = validate_edit_ops_json(ops.as_array().unwrap()).unwrap_err();
        assert_eq!(e.code, "UNKNOWN_OP_FIELD");
        assert!(
            e.message.contains("did you mean 'beforeId'"),
            "msg: {}",
            e.message
        );
    }

    #[test]
    fn validate_edit_ops_hints_view_id_belongs_under_target_for_add() {
        let ops = json!([
            {"kind": "add_layout_node", "node": {"type": "block", "id": "n", "blockId": "b"},
             "viewId": "detail"}
        ]);
        let e = validate_edit_ops_json(ops.as_array().unwrap()).unwrap_err();
        assert_eq!(e.code, "UNKNOWN_OP_FIELD");
        assert!(
            e.message.contains("viewId") && e.message.contains("`target`"),
            "msg: {}",
            e.message
        );
    }

    #[test]
    fn validate_edit_ops_rejects_missing_and_unknown_kind() {
        let missing = json!([{ "nodeId": "n" }]);
        let e = validate_edit_ops_json(missing.as_array().unwrap()).unwrap_err();
        assert_eq!(e.code, "MISSING_OP_KIND");
        let unknown = json!([{ "kind": "frobnicate", "nodeId": "n" }]);
        let e = validate_edit_ops_json(unknown.as_array().unwrap()).unwrap_err();
        assert_eq!(e.code, "UNKNOWN_OP_KIND");
    }

    #[test]
    fn deny_unknown_fields_rejects_typo_inside_target() {
        // Nested typo inside a correctly-placed `target` is caught by
        // deny_unknown_fields on LayoutTarget at typed deserialization.
        let bad: Result<ReportEditOp, _> = serde_json::from_value(json!({
            "kind": "add_layout_node",
            "node": {"type": "block", "id": "n", "blockId": "b"},
            "target": {"beforeNodeId": "x"}
        }));
        assert!(
            bad.is_err(),
            "expected deny_unknown_fields to reject beforeNodeId under target"
        );
    }

    // ----- remove/move item-wrapper-id hint -----

    #[test]
    fn remove_layout_node_item_id_yields_child_node_hint() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": {"id": "root", "items": [
                {"id": "wrap_1", "child": {"type": "block", "id": "node_1", "blockId": "b1"}}
            ]}
        }))
        .unwrap();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "wrap_1".to_string(),
                view_id: None,
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "LAYOUT_NODE_NOT_FOUND");
        assert!(e.message.contains("item-wrapper id"), "msg: {}", e.message);
        assert!(e.message.contains("node_1"), "msg: {}", e.message);
    }

    // ----- view-layout targeting -----

    fn def_with_view() -> ReportDefinition {
        serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [
                {"id": "a", "type": "markdown", "markdown": {"content": "A"}},
                {"id": "b", "type": "markdown", "markdown": {"content": "B"}},
                {"id": "c", "type": "markdown", "markdown": {"content": "C"}}
            ],
            "layout": {"id": "root", "columns": 1, "items": [
                {"id": "root_i0", "child": {"type": "block", "id": "a_node", "blockId": "a"}}
            ]},
            "views": [
                {"id": "detail", "title": "Detail", "layout": {"id": "detail_root", "columns": 1, "items": [
                    {"id": "dv_i0", "child": {"type": "block", "id": "dv_b", "blockId": "b"}}
                ]}}
            ]
        }))
        .unwrap()
    }

    fn view_node_ids(def: &ReportDefinition, view_id: &str) -> Vec<String> {
        def.views
            .iter()
            .find(|v| v.id == view_id)
            .unwrap()
            .layout
            .items
            .iter()
            .map(|i| layout_node_id(&i.child).to_string())
            .collect()
    }

    #[test]
    fn add_layout_node_targets_view_via_view_id() {
        let mut def = def_with_view();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(
                    json!({"type": "block", "id": "c_node", "blockId": "c"}),
                )
                .unwrap(),
                target: LayoutTarget {
                    view_id: Some("detail".to_string()),
                    before_id: Some("dv_b".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        // Landed in the detail view, before dv_b — not in root.
        assert_eq!(view_node_ids(&def, "detail"), ["c_node", "dv_b"]);
        assert_eq!(
            def.layout
                .items
                .iter()
                .map(|i| layout_node_id(&i.child).to_string())
                .collect::<Vec<_>>(),
            ["a_node"]
        );
    }

    #[test]
    fn add_layout_node_unknown_view_errors() {
        let mut def = def_with_view();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(
                    json!({"type": "block", "id": "c_node", "blockId": "c"}),
                )
                .unwrap(),
                target: LayoutTarget {
                    view_id: Some("nope".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "VIEW_NOT_FOUND");
    }

    #[test]
    fn global_uniqueness_rejects_id_colliding_with_view_node() {
        // Adding to ROOT a node whose id already exists in a VIEW is rejected.
        let mut def = def_with_view();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(
                    json!({"type": "block", "id": "dv_b", "blockId": "c"}),
                )
                .unwrap(),
                target: LayoutTarget::default(),
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "DUPLICATE_LAYOUT_NODE_ID");
    }

    #[test]
    fn remove_layout_node_targets_view() {
        let mut def = def_with_view();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "dv_b".to_string(),
                view_id: Some("detail".to_string()),
            }],
        )
        .unwrap();
        assert!(view_node_ids(&def, "detail").is_empty());
    }

    #[test]
    fn remove_layout_node_view_root_is_rejected() {
        let mut def = def_with_view();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "detail_root".to_string(),
                view_id: Some("detail".to_string()),
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "CANNOT_REMOVE_ROOT_GRID");
    }

    #[test]
    fn move_layout_node_cross_tree_is_explicit_error() {
        // Node lives in root; target is the detail view -> CROSS_TREE_MOVE_UNSUPPORTED.
        let mut def = def_with_view();
        let e = apply_edit_ops(
            &mut def,
            &[ReportEditOp::MoveLayoutNode {
                node_id: "a_node".to_string(),
                target: LayoutTarget {
                    view_id: Some("detail".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, "CROSS_TREE_MOVE_UNSUPPORTED");
    }

    #[test]
    fn patch_layout_node_targets_view_node() {
        let mut def = def_with_view();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::PatchLayoutNode {
                node_id: "detail_root".to_string(),
                patch: json!({"columns": 2}),
                view_id: Some("detail".to_string()),
            }],
        )
        .unwrap();
        let detail = def.views.iter().find(|v| v.id == "detail").unwrap();
        assert_eq!(detail.layout.columns, Some(2));
        // Root layout columns untouched.
        assert_eq!(def.layout.columns, Some(1));
    }

    #[test]
    fn absent_view_id_still_targets_root_identically() {
        // Backward-compat: no viewId behaves exactly as before (root layout).
        let mut def = def_with_view();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::AddLayoutNode {
                node: serde_json::from_value(
                    json!({"type": "block", "id": "c_node", "blockId": "c"}),
                )
                .unwrap(),
                target: LayoutTarget::default(),
            }],
        )
        .unwrap();
        assert_eq!(
            def.layout
                .items
                .iter()
                .map(|i| layout_node_id(&i.child).to_string())
                .collect::<Vec<_>>(),
            ["a_node", "c_node"]
        );
        // View untouched.
        assert_eq!(view_node_ids(&def, "detail"), ["dv_b"]);
    }
}
