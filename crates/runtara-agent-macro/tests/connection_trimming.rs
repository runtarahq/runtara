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

    #[field(display_name = "API Key", secret, trim)]
    api_key: String,

    #[field(display_name = "Password", secret)]
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
fn fields_trim_only_when_explicitly_marked() {
    assert!(trims("api_key"), "`trim` opts a secret into normalization");
    assert!(!trims("password"), "an unannotated secret is kept as typed");
    assert!(trims("client_id"), "`trim` trims a readable field");
    assert!(
        !trims("base_url"),
        "a readable field is kept as typed by default"
    );
}
