// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A credential for one conversation (`docs/native-chat-phase-4.md`, 4.6).
//!
//! A chat embedded in someone's page, for their end user, has nothing safe to
//! hold: a tenant's API key or a member's sign-in would let the page do
//! anything they may. A conversation token is made by a member who may run
//! the workflow, for one conversation, for a short while. It acts as that
//! member, held to reading and sending in that conversation and nothing else
//! (`middleware::auth`, and again by the engine, `engine::wire`), and every
//! message sent with it is from the sender fixed when it was made.
//!
//! Only its hash is kept, as an API key's is.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use utoipa::ToSchema;

use crate::api::services::conversations::{ConversationError, ConversationResult, Sender};

/// What a conversation token is written as: `rtc_` and 64 hex digits.
pub const PREFIX: &str = "rtc_";
/// How long a token lasts unless asked otherwise, and at most.
pub const LASTS: Duration = Duration::from_secs(60 * 60);
pub const LASTS_AT_MOST: Duration = Duration::from_secs(24 * 60 * 60);
const LASTS_AT_LEAST: Duration = Duration::from_secs(60);

/// What is asked when a token is made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssueToken {
    /// How long it lasts, in seconds: 3600 unless given, 60 to 86400.
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    /// Who every message sent with it is from. A page that holds it cannot
    /// say otherwise.
    #[serde(default)]
    pub sender: Option<Sender>,
}

/// A token as it is handed over: once, and never again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedToken {
    /// Present it as `Authorization: Bearer <token>`.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// What a token grants, as its row says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenGrant {
    pub tenant_id: String,
    pub workflow_id: String,
    pub key: String,
    #[serde(default)]
    pub sender: Option<Sender>,
    pub issued_by: String,
    /// The first few digits of its hash: how logs and audit name it.
    pub token_id: String,
}

/// The hash a token is kept and found by.
pub fn hash_of(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

/// Makes a token for the conversation `key` names with `workflow`, in the
/// caller's tenant (the scope it runs in). The conversation is opened if it
/// was not.
pub async fn issue(
    pool: &PgPool,
    tenant: &str,
    workflow: &str,
    key: &str,
    issued_by: &str,
    issue: IssueToken,
) -> ConversationResult<IssuedToken> {
    let lasts = Duration::from_secs(issue.ttl_seconds.unwrap_or(LASTS.as_secs()));
    if !(LASTS_AT_LEAST..=LASTS_AT_MOST).contains(&lasts) {
        return Err(ConversationError::Invalid(format!(
            "a token lasts {} to {} seconds",
            LASTS_AT_LEAST.as_secs(),
            LASTS_AT_MOST.as_secs()
        )));
    }
    let random: [u8; 32] = rand::random();
    let token = format!("{PREFIX}{}", hex::encode(random));
    let expires_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO conversation_tokens \
             (token_hash, tenant_id, workflow_id, key, sender, issued_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7)) \
         RETURNING expires_at",
    )
    .bind(hash_of(&token))
    .bind(tenant)
    .bind(workflow)
    .bind(key)
    .bind(
        issue
            .sender
            .map(|sender| serde_json::to_value(sender).unwrap_or_default()),
    )
    .bind(issued_by)
    .bind(lasts.as_secs() as f64)
    .fetch_one(pool)
    .await
    .map_err(|error| ConversationError::Storage(error.to_string()))?;
    Ok(IssuedToken { token, expires_at })
}

/// What the token whose hash is `token_hash` grants, if it has not
/// expired. Looked for across tenants: the token is all its holder shows.
pub async fn by_hash(pool: &PgPool, token_hash: &str) -> Result<TokenGrant, String> {
    let row = sqlx::query(
        "SELECT tenant_id, workflow_id, key, sender, issued_by FROM conversation_tokens \
         WHERE token_hash = $1 AND expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "no such conversation token, or it has expired".to_owned())?;
    let sender: Option<serde_json::Value> = row.try_get("sender").map_err(|e| e.to_string())?;
    Ok(TokenGrant {
        tenant_id: row.try_get("tenant_id").map_err(|e| e.to_string())?,
        workflow_id: row.try_get("workflow_id").map_err(|e| e.to_string())?,
        key: row.try_get("key").map_err(|e| e.to_string())?,
        sender: sender.and_then(|sender| serde_json::from_value(sender).ok()),
        issued_by: row.try_get("issued_by").map_err(|e| e.to_string())?,
        token_id: token_hash.chars().take(12).collect(),
    })
}

/// Removes the tokens that have expired, across tenants.
pub async fn sweep(pool: &PgPool) -> Result<u64, sqlx::Error> {
    sqlx::query("DELETE FROM conversation_tokens WHERE expires_at <= now()")
        .execute(pool)
        .await
        .map(|done| done.rows_affected())
}
