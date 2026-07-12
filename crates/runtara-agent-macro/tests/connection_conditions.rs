// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use runtara_agent_macro::ConnectionParams;
use runtara_dsl::form::{analyze_form, connection_form_definition, field_equals, not};
use serde_json::json;

fn bearer_mode() -> runtara_dsl::ConditionExpression {
    field_equals("auth_mode", "bearer")
}

fn not_disabled() -> runtara_dsl::ConditionExpression {
    not(field_equals("auth_mode", "disabled"))
}

#[allow(dead_code)]
#[derive(ConnectionParams)]
#[connection(
    integration_id = "macro_condition_fixture",
    display_name = "Macro Condition Fixture"
)]
struct ConditionalParams {
    #[field(
        display_name = "Auth mode",
        default = "none",
        enum_values = "none,bearer,disabled"
    )]
    auth_mode: String,

    #[field(
        display_name = "Token",
        secret,
        clearable,
        requires_reauthorization,
        visible = bearer_mode,
        enabled = not_disabled,
        required = bearer_mode
    )]
    token: Option<String>,
}

#[test]
fn derive_emits_all_canonical_condition_factories() {
    let definition = connection_form_definition(&__CONNECTION_META_ConditionalParams);
    let token = &definition.fields["token"];

    assert!(token.conditions.visible.is_some());
    assert!(token.conditions.enabled.is_some());
    assert!(token.conditions.required.is_some());
    let token_meta = __CONNECTION_META_ConditionalParams
        .fields
        .iter()
        .find(|field| field.name == "token")
        .unwrap();
    assert!(token_meta.behavior.clearable);
    assert!(token_meta.behavior.requires_reauthorization);

    let none = analyze_form(&definition, &json!({ "auth_mode": "none" }));
    assert!(!none.fields["token"].visible);
    assert!(none.fields["token"].enabled);
    assert!(none.valid);

    let bearer_missing = analyze_form(&definition, &json!({ "auth_mode": "bearer" }));
    assert!(bearer_missing.fields["token"].visible);
    assert!(bearer_missing.fields["token"].required);
    assert!(!bearer_missing.valid);

    let disabled = analyze_form(&definition, &json!({ "auth_mode": "disabled" }));
    assert!(!disabled.fields["token"].visible);
    assert!(!disabled.fields["token"].enabled);
}
