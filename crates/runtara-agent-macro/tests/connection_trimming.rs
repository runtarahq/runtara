// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use runtara_agent_macro::ConnectionParams;

#[allow(dead_code)]
#[derive(ConnectionParams)]
#[connection(
    integration_id = "macro_trimming_fixture",
    display_name = "Macro Trimming Fixture"
)]
struct TrimmingParams {
    #[field(display_name = "Base URL")]
    base_url: String,

    #[field(display_name = "Client ID", trim)]
    client_id: String,

    #[field(display_name = "API Key", secret)]
    api_key: String,

    #[field(display_name = "Password", secret, keep_whitespace)]
    password: String,
}

fn trims(name: &str) -> bool {
    __CONNECTION_META_TrimmingParams
        .fields
        .iter()
        .find(|field| field.name == name)
        .unwrap()
        .trims
}

#[test]
fn a_secret_trims_unless_kept_and_a_readable_field_only_when_marked() {
    assert!(trims("api_key"), "a secret trims by default");
    assert!(
        !trims("password"),
        "`keep_whitespace` keeps a secret as typed"
    );
    assert!(trims("client_id"), "`trim` trims a readable field");
    assert!(
        !trims("base_url"),
        "a readable field is kept as typed by default"
    );
}
