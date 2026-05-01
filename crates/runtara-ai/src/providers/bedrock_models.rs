// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checked-in Amazon Bedrock model metadata for AI Agent configuration.
//!
//! This is product metadata, not tenant/account model access state. Release
//! tooling can refresh the JSON from AWS, but normal builds and runtime do not
//! require AWS credentials.

use serde_json::Value;

pub const BEDROCK_MODELS_JSON: &str = include_str!("bedrock_models.generated.json");

pub fn catalog_json() -> Value {
    serde_json::from_str(BEDROCK_MODELS_JSON).unwrap_or_else(|_| {
        serde_json::json!({
            "generatedAt": null,
            "source": "invalid embedded Bedrock model registry",
            "models": []
        })
    })
}
