// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Capabilities written by hand, listed under `custom_capabilities` in
//! `connector.yaml`, which adds them to the agent beside the generated ones.
//!
//! Search Entities searches any of Shopware's records. Every one is searched
//! the same way (`POST /api/search/<entity>` with the same criteria), and a
//! create needs the IDs of records of their own: a product's tax and
//! currency, a customer's group, sales channel and country, an order's
//! deliveries and payments. One step reads them all, where a generated search
//! per record would add a type for each.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kit::{self, AgentError, RawConnection};

const PREFIX: &str = "SHOPWARE";

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Search Entities")]
pub struct SearchEntitiesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Entity",
        description = "The kind of record, as the Admin API names it: `tax`, `currency`, `sales-channel`, `customer-group`, `country`, `salutation`, `category`, `product-manufacturer`, `order-delivery`, `order-transaction`, `state-machine-state`, … (`sales_channel` is read as `sales-channel`).",
        example = "tax"
    )]
    pub entity: String,

    #[field(
        display_name = "Filter",
        description = "Filters a record must match, such as `[{\"type\": \"equals\", \"field\": \"isoCode\", \"value\": \"EUR\"}]`: `equals`, `equalsAny`, `contains`, `prefix`, `range`, `not`, `multi`, as in Shopware's search criteria."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Vec<Value>>,

    #[field(
        display_name = "Sort",
        description = "The order of the records, such as `[{\"field\": \"name\", \"order\": \"ASC\"}]`."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Value>>,

    #[field(
        display_name = "Term",
        description = "Text to search the records' searchable fields for."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,

    #[field(display_name = "IDs", description = "The IDs of the records to read.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,

    #[field(
        display_name = "Limit",
        description = "The most records to return in one page.",
        example = "25"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,

    #[field(display_name = "Page", description = "The page to return, from 1.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,

    #[field(
        display_name = "Associations",
        description = "What the records refer to that should come back with them, each with criteria of its own: `{\"addresses\": {}}`."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associations: Option<Value>,

    #[field(
        display_name = "Includes",
        description = "The only fields to return, per kind of record: `{\"tax\": [\"id\", \"name\", \"taxRate\"]}`."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub includes: Option<Value>,

    #[field(
        display_name = "Total Count Mode",
        description = "How `total` is counted, as in the other searches: `exact` counts every match (slower); `next-pages` counts only as far as the next pages; `none`, the default, counts only this page's records. Shopware reads a value it does not know as `none`.",
        example = "exact"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_count_mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Search Entities Output")]
pub struct SearchEntitiesOutput {
    #[field(
        display_name = "Total",
        description = "How many records match, as `total_count_mode` counts them: only this page's unless it says otherwise."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,

    #[field(
        display_name = "Data",
        description = "The records of this page, as Shopware sends them."
    )]
    #[serde(default)]
    pub data: Vec<Value>,

    #[field(
        display_name = "Aggregations",
        description = "What the search's aggregations work out."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregations: Option<Value>,
}

impl SearchEntitiesInput {
    /// The request this capability sends; the connection supplies host and
    /// credentials.
    pub fn request(&self) -> Result<kit::Request, AgentError> {
        let entity = entity_path(&self.entity)?;
        let body = kit::Body::new()
            .field_opt("filter", &self.filter)
            .field_opt("sort", &self.sort)
            .field_opt("term", &self.term)
            .field_opt("ids", &self.ids)
            .field_opt("limit", &self.limit)
            .field_opt("page", &self.page)
            .field_opt("associations", &self.associations)
            .field_opt("includes", &self.includes)
            .field_opt("total-count-mode", &self.total_count_mode);
        kit::Request::new("POST", format!("/api/search/{entity}")).json_body(body, PREFIX)
    }
}

/// The path segment of a kind of record: Shopware's entity name with `-` for
/// `_` (`sales_channel` is `sales-channel`), lower-case letters, digits and
/// dashes only, so a step cannot reach another route.
pub fn entity_path(entity: &str) -> Result<String, AgentError> {
    let name = entity.trim().replace('_', "-");
    let valid = !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(name)
    } else {
        Err(AgentError::permanent(
            format!("{PREFIX}_INVALID_REQUEST"),
            format!(
                "`{}` is no entity name: use the Admin API's, such as `tax` or `sales-channel`",
                entity.trim()
            ),
        ))
    }
}

/// Searches the records of one kind with Shopware's search criteria.
#[capability(
    module = "shopware",
    display_name = "Search Entities",
    description = "Searches any kind of Shopware record (`entity`: `tax`, `currency`, `sales-channel`, `customer-group`, `country`, `order-delivery`, …) with Shopware's search criteria, a page at a time. Finds the IDs other steps need: a product's `taxId` and price `currencyId`, a customer's `groupId`, `salesChannelId` and `countryId`."
)]
pub async fn search_entities(
    input: SearchEntitiesInput,
) -> Result<SearchEntitiesOutput, AgentError> {
    let connection = kit::require_connection(&input._connection, PREFIX)?;
    input.request()?.send(connection, PREFIX).await
}
