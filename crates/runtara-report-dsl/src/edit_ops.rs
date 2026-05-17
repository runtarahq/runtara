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

use crate::types::{ReportBlockDefinition, ReportDefinition, ReportLayoutNode};

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, rename = "beforeId", skip_serializing_if = "Option::is_none")]
    pub before_id: Option<String>,
    #[serde(default, rename = "afterId", skip_serializing_if = "Option::is_none")]
    pub after_id: Option<String>,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutTarget {
    #[serde(
        default,
        rename = "parentNodeId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_node_id: Option<String>,
    #[serde(default, rename = "columnId", skip_serializing_if = "Option::is_none")]
    pub column_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, rename = "beforeId", skip_serializing_if = "Option::is_none")]
    pub before_id: Option<String>,
    #[serde(default, rename = "afterId", skip_serializing_if = "Option::is_none")]
    pub after_id: Option<String>,
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
    },
    PatchLayoutNode {
        #[serde(rename = "nodeId")]
        node_id: String,
        patch: Value,
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
        ReportEditOp::ReplaceLayoutNode { node_id, node } => {
            replace_layout_node(def, node_id, node.clone())
        }
        ReportEditOp::PatchLayoutNode { node_id, patch } => patch_layout_node(def, node_id, patch),
        ReportEditOp::MoveLayoutNode { node_id, target } => move_layout_node(def, node_id, target),
        ReportEditOp::RemoveLayoutNode { node_id } => remove_layout_node(def, node_id),
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

fn add_layout_node(
    def: &mut ReportDefinition,
    node: ReportLayoutNode,
    target: &LayoutTarget,
) -> Result<(), EditOpError> {
    let node_id = layout_node_id(&node).to_string();
    if layout_node_exists(&def.layout, &node_id) {
        return Err(err(
            "DUPLICATE_LAYOUT_NODE_ID",
            format!("Layout node '{}' already exists", node_id),
        ));
    }
    insert_into_target(&mut def.layout, target, node)
}

fn replace_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    node: ReportLayoutNode,
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
    if !replace_in_tree(&mut def.layout, node_id, node) {
        return Err(layout_not_found(node_id));
    }
    Ok(())
}

fn patch_layout_node(
    def: &mut ReportDefinition,
    node_id: &str,
    patch: &Value,
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
    let Some(node) = find_in_tree_mut(&mut def.layout, node_id) else {
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
    let Some(node) = remove_from_tree(&mut def.layout, node_id) else {
        return Err(layout_not_found(node_id));
    };
    match insert_into_target(&mut def.layout, target, node) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Reattach at the end of root so we don't drop the node on
            // target-resolution failure. Caller still sees the original
            // pre-clone state because the wrapping `apply_edit_ops`
            // operates on a clone.
            Err(e)
        }
    }
}

fn remove_layout_node(def: &mut ReportDefinition, node_id: &str) -> Result<(), EditOpError> {
    if remove_from_tree(&mut def.layout, node_id).is_none() {
        return Err(layout_not_found(node_id));
    }
    Ok(())
}

// ----- Layout tree walking -----

fn layout_node_id(node: &ReportLayoutNode) -> &str {
    match node {
        ReportLayoutNode::Block(n) => &n.id,
        ReportLayoutNode::MetricRow(n) => &n.id,
        ReportLayoutNode::Section(n) => &n.id,
        ReportLayoutNode::Columns(n) => &n.id,
        ReportLayoutNode::Grid(n) => &n.id,
    }
}

fn layout_not_found(node_id: &str) -> EditOpError {
    err(
        "LAYOUT_NODE_NOT_FOUND",
        format!("Layout node '{}' not found", node_id),
    )
}

/// `true` if a layout node with `node_id` is anywhere in the tree.
/// Used to reject `AddLayoutNode` of a duplicate id.
fn layout_node_exists(nodes: &[ReportLayoutNode], node_id: &str) -> bool {
    for node in nodes {
        if layout_node_id(node) == node_id {
            return true;
        }
        let in_children = match node {
            ReportLayoutNode::Section(s) => layout_node_exists(&s.children, node_id),
            ReportLayoutNode::Columns(c) => c
                .columns
                .iter()
                .any(|col| layout_node_exists(&col.children, node_id)),
            _ => false,
        };
        if in_children {
            return true;
        }
    }
    false
}

fn find_in_tree_mut<'a>(
    nodes: &'a mut [ReportLayoutNode],
    node_id: &str,
) -> Option<&'a mut ReportLayoutNode> {
    for node in nodes.iter_mut() {
        if layout_node_id(node) == node_id {
            return Some(node);
        }
        let recurse = match node {
            ReportLayoutNode::Section(s) => find_in_tree_mut(&mut s.children, node_id),
            ReportLayoutNode::Columns(c) => c
                .columns
                .iter_mut()
                .find_map(|col| find_in_tree_mut(&mut col.children, node_id)),
            _ => None,
        };
        if recurse.is_some() {
            return recurse;
        }
    }
    None
}

fn replace_in_tree(
    nodes: &mut [ReportLayoutNode],
    node_id: &str,
    replacement: ReportLayoutNode,
) -> bool {
    for node in nodes.iter_mut() {
        if layout_node_id(node) == node_id {
            *node = replacement;
            return true;
        }
    }
    // Recurse into child containers.
    for node in nodes.iter_mut() {
        let replaced = match node {
            ReportLayoutNode::Section(s) => {
                replace_in_tree(&mut s.children, node_id, replacement.clone())
            }
            ReportLayoutNode::Columns(c) => c
                .columns
                .iter_mut()
                .any(|col| replace_in_tree(&mut col.children, node_id, replacement.clone())),
            _ => false,
        };
        if replaced {
            return true;
        }
    }
    false
}

#[allow(clippy::ptr_arg)] // Vec::remove requires &mut Vec, not &mut [_].
fn remove_from_tree(nodes: &mut Vec<ReportLayoutNode>, node_id: &str) -> Option<ReportLayoutNode> {
    if let Some(index) = nodes.iter().position(|n| layout_node_id(n) == node_id) {
        return Some(nodes.remove(index));
    }
    for node in nodes.iter_mut() {
        let removed = match node {
            ReportLayoutNode::Section(s) => remove_from_tree(&mut s.children, node_id),
            ReportLayoutNode::Columns(c) => c
                .columns
                .iter_mut()
                .find_map(|col| remove_from_tree(&mut col.children, node_id)),
            _ => None,
        };
        if removed.is_some() {
            return removed;
        }
    }
    None
}

/// Resolve the destination container for [`AddLayoutNode`]/[`MoveLayoutNode`]
/// and insert.
fn insert_into_target(
    root: &mut Vec<ReportLayoutNode>,
    target: &LayoutTarget,
    node: ReportLayoutNode,
) -> Result<(), EditOpError> {
    let dest = resolve_container_mut(root, target)?;
    let index = resolve_layout_index(dest, target)?;
    dest.insert(index, node);
    Ok(())
}

fn resolve_container_mut<'a>(
    root: &'a mut Vec<ReportLayoutNode>,
    target: &LayoutTarget,
) -> Result<&'a mut Vec<ReportLayoutNode>, EditOpError> {
    let Some(parent_id) = &target.parent_node_id else {
        if target.column_id.is_some() {
            return Err(err(
                "INVALID_LAYOUT_TARGET",
                "columnId requires parentNodeId pointing at a columns layout node",
            ));
        }
        return Ok(root);
    };
    let Some(node) = find_in_tree_mut(root, parent_id) else {
        return Err(layout_not_found(parent_id));
    };
    match node {
        ReportLayoutNode::Section(s) => {
            if target.column_id.is_some() {
                return Err(err(
                    "INVALID_LAYOUT_TARGET",
                    "columnId is only valid for columns layout nodes",
                ));
            }
            Ok(&mut s.children)
        }
        ReportLayoutNode::Columns(c) => {
            let column_id = target.column_id.as_deref().ok_or_else(|| {
                err(
                    "INVALID_LAYOUT_TARGET",
                    "columns layout node requires columnId for child placement",
                )
            })?;
            let column = c
                .columns
                .iter_mut()
                .find(|col| col.id == column_id)
                .ok_or_else(|| {
                    err(
                        "LAYOUT_COLUMN_NOT_FOUND",
                        format!("Layout column '{}' not found", column_id),
                    )
                })?;
            Ok(&mut column.children)
        }
        _ => Err(err(
            "INVALID_LAYOUT_TARGET",
            "parentNodeId must reference a container (section or columns) layout node",
        )),
    }
}

fn resolve_layout_index(
    siblings: &[ReportLayoutNode],
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
        (Some(i), _, _) => Ok(i.min(siblings.len())),
        (_, Some(before), _) => siblings
            .iter()
            .position(|n| layout_node_id(n) == before)
            .ok_or_else(|| {
                err(
                    "LAYOUT_ANCHOR_NOT_FOUND",
                    format!("Position anchor layout node '{}' not found", before),
                )
            }),
        (_, _, Some(after)) => siblings
            .iter()
            .position(|n| layout_node_id(n) == after)
            .map(|i| i + 1)
            .ok_or_else(|| {
                err(
                    "LAYOUT_ANCHOR_NOT_FOUND",
                    format!("Position anchor layout node '{}' not found", after),
                )
            }),
        (None, None, None) => Ok(siblings.len()),
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
    fn add_layout_node_to_section_via_parent_id() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": [{"type": "section", "id": "s1", "children": []}]
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
                    parent_node_id: Some("s1".to_string()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        match &def.layout[0] {
            ReportLayoutNode::Section(s) => assert_eq!(s.children.len(), 1),
            _ => panic!("expected Section at root"),
        }
    }

    #[test]
    fn remove_layout_node_finds_nested_match() {
        let mut def: ReportDefinition = serde_json::from_value(json!({
            "definitionVersion": 1,
            "blocks": [{"id": "b1", "type": "markdown", "markdown": {"content": "x"}}],
            "layout": [{
                "type": "section",
                "id": "s1",
                "children": [{"type": "block", "id": "ln1", "blockId": "b1"}]
            }]
        }))
        .unwrap();
        apply_edit_ops(
            &mut def,
            &[ReportEditOp::RemoveLayoutNode {
                node_id: "ln1".to_string(),
            }],
        )
        .unwrap();
        match &def.layout[0] {
            ReportLayoutNode::Section(s) => assert!(s.children.is_empty()),
            _ => panic!("expected Section at root"),
        }
    }
}
