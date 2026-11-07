// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection types declared by agents, registered at boot.
//!
//! Agents declare derived connection types in their `meta.json`
//! ([`runtara_dsl::derived_connection`]). The server registers the
//! declarations of the agents it loaded with [`register`]; from then on
//! [`crate::registry`] lists and finds them like compiled types, and
//! [`to_base`] turns a derived connection's parameters into its base type's
//! for authentication.
//!
//! The registry is process-wide, like the compiled one: the agent release is
//! fixed for the life of a server process.

use std::sync::{Arc, RwLock};

use runtara_dsl::agent_meta::{ConnectionCategory, ConnectionFieldMeta, ConnectionTypeMeta};
use runtara_dsl::derived_connection::{BASE_URL_FIELD, DerivedConnectionType};
use serde_json::Value;

/// A registered derived connection type.
#[derive(Debug)]
pub struct DerivedType {
    pub declaration: DerivedConnectionType,
    /// Its descriptor, as the compiled types have one.
    pub meta: &'static ConnectionTypeMeta,
    /// The base type's descriptor.
    pub base: &'static ConnectionTypeMeta,
    /// The agents that declare it (one or more, with identical declarations).
    pub agents: Vec<String>,
}

static REGISTRY: RwLock<Vec<Arc<DerivedType>>> = RwLock::new(Vec::new());

/// Replace the registered derived types with `declarations`, given as
/// `(declaring agent id, declaration)`. All or nothing: on any invalid
/// declaration nothing changes and the error names it.
///
/// Refused: a declaration that fails
/// [`DerivedConnectionType::validate_shape`] or does not fit its base; an id
/// a compiled type already has; one id declared differently by two agents.
pub fn register(declarations: Vec<(String, DerivedConnectionType)>) -> Result<usize, String> {
    let mut built: Vec<DerivedType> = Vec::new();
    for (agent, declaration) in declarations {
        let context = |e: String| format!("agent `{agent}`: {e}");
        declaration.validate_shape().map_err(context)?;
        let id = declaration.integration_id.as_str();
        if crate::registry::find_compiled_connection_type(id).is_some() {
            return Err(context(format!(
                "connection type `{id}` is already a built-in connection type"
            )));
        }
        if let Some(existing) = built
            .iter_mut()
            .find(|t| t.declaration.integration_id == id)
        {
            if existing.declaration != declaration {
                return Err(context(format!(
                    "connection type `{id}` is also declared, differently, by agent `{}`",
                    existing.agents.join("`, `")
                )));
            }
            existing.agents.push(agent);
            continue;
        }
        let base = crate::registry::find_compiled_connection_type(&declaration.based_on)
            .ok_or_else(|| {
                context(format!(
                    "connection type `{id}`: base `{}` is not registered",
                    declaration.based_on
                ))
            })?;
        let base_fields: Vec<&str> = base.fields.iter().map(|f| f.name).collect();
        declaration
            .check_against_base(&base_fields)
            .map_err(context)?;
        built.push(DerivedType {
            meta: materialize(&declaration, base),
            base,
            declaration,
            agents: vec![agent],
        });
    }
    let count = built.len();
    *REGISTRY.write().unwrap_or_else(|e| e.into_inner()) =
        built.into_iter().map(Arc::new).collect();
    Ok(count)
}

/// Every registered derived type.
pub fn all() -> Vec<Arc<DerivedType>> {
    REGISTRY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The derived type `integration_id`, if one is registered.
pub fn find(integration_id: &str) -> Option<Arc<DerivedType>> {
    REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|t| t.declaration.integration_id == integration_id)
        .cloned()
}

/// For a derived type, its base type's id and the base parameters of a
/// connection stored with `params`; `None` for any other type. An `Err`
/// (a missing or invalid param) means the connection cannot authenticate:
/// callers fail closed.
pub fn to_base(
    integration_id: &str,
    params: &Value,
) -> Option<Result<(&'static str, Value), String>> {
    let derived = find(integration_id)?;
    Some(
        derived
            .declaration
            .to_base_parameters(params)
            .map(|base| (derived.base.integration_id, base)),
    )
}

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

fn leak_opt(text: &Option<String>) -> Option<&'static str> {
    text.as_deref().map(leak)
}

/// The descriptor of `declaration`: its params first, then the base's user
/// fields (minus `base_url` and the fixed ones) with their new labels.
/// Leaked once per registration: descriptors are `&'static` throughout.
fn materialize(
    declaration: &DerivedConnectionType,
    base: &'static ConnectionTypeMeta,
) -> &'static ConnectionTypeMeta {
    let mut fields: Vec<ConnectionFieldMeta> = declaration
        .params
        .iter()
        .enumerate()
        .map(|(order, param)| ConnectionFieldMeta {
            name: leak(&param.name),
            type_name: "String",
            is_optional: false,
            display_name: Some(leak(&param.display_name)),
            description: leak_opt(&param.description),
            placeholder: leak_opt(&param.placeholder),
            order: order as i32,
            default_value: None,
            is_secret: false,
            enum_values: None,
            is_url: false,
            is_required: true,
            control: None,
            section: None,
            access: Default::default(),
            conditions: Default::default(),
            behavior: Default::default(),
        })
        .collect();
    let offset = fields.len() as i32;
    for field in base.fields {
        if field.name == BASE_URL_FIELD || declaration.fixed.contains_key(field.name) {
            continue;
        }
        let mut field = field.clone();
        field.order += offset;
        if let Some(label) = declaration.fields.iter().find(|l| l.name == field.name) {
            if let Some(display_name) = &label.display_name {
                field.display_name = Some(leak(display_name));
            }
            if let Some(description) = &label.description {
                field.description = Some(leak(description));
            }
            if let Some(placeholder) = &label.placeholder {
                field.placeholder = Some(leak(placeholder));
            }
        }
        fields.push(field);
    }
    let fields: &'static [ConnectionFieldMeta] = Box::leak(fields.into_boxed_slice());
    Box::leak(Box::new(ConnectionTypeMeta {
        integration_id: leak(&declaration.integration_id),
        display_name: leak(&declaration.display_name),
        description: leak_opt(&declaration.description).or(base.description),
        category: declaration
            .category
            .as_deref()
            .and_then(ConnectionCategory::parse)
            .or(base.category),
        service_id: Some(leak(&declaration.integration_id)),
        auth_type: base.auth_type,
        fields,
        sections: base.sections,
        oauth_config: None,
        named_endpoints: &[],
    }))
}
