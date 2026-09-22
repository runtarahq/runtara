// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Statically compiled connection descriptors shared by host and WASM metadata consumers.

use runtara_dsl::agent_meta::ConnectionTypeMeta;

pub static CONNECTION_TYPES: &[&ConnectionTypeMeta] = &[
    &crate::extractors::http_api_key::__CONNECTION_META_HttpApiKeyParams,
    &crate::extractors::http_bearer::__CONNECTION_META_HttpBearerParams,
    &crate::extractors::http_mtls::__CONNECTION_META_HttpMtlsParams,
    &crate::extractors::connection_types::__CONNECTION_META_ShopifyAccessTokenParams,
    &crate::extractors::connection_types::__CONNECTION_META_ShopifyClientCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_OpenAiApiKeyParams,
    &crate::extractors::connection_types::__CONNECTION_META_AwsCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_TelegramBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_SlackBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_TeamsBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_MicrosoftEntraClientCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_HttpOAuth2ClientCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_HttpOAuth2AuthorizationCodeParams,
    &crate::extractors::connection_types::__CONNECTION_META_MailgunParams,
    &crate::extractors::connection_types::__CONNECTION_META_HubSpotPrivateAppParams,
    &crate::extractors::connection_types::__CONNECTION_META_HubSpotAccessTokenParams,
    &crate::extractors::connection_types::__CONNECTION_META_QuickBooksOnlineParams,
    &crate::extractors::connection_types::__CONNECTION_META_PostgresDatabaseParams,
    &crate::extractors::connection_types::__CONNECTION_META_S3CompatibleParams,
    &crate::extractors::connection_types::__CONNECTION_META_AzureBlobStorageParams,
    &crate::extractors::connection_types::__CONNECTION_META_StripeApiKeyParams,
    &crate::extractors::connection_types::__CONNECTION_META_McpConnectionParams,
];
