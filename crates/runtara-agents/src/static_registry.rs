// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Statically compiled agent metadata registry.
//!
//! This is the single discovery source for runtara-agents. The entries are
//! explicit so native and WASM builds see the same metadata for the features
//! they compile.

#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
use runtara_dsl::agent_meta::CapabilityExecutor;
use runtara_dsl::agent_meta::{
    AgentModuleConfig, CapabilityMeta, ConnectionTypeMeta, InputTypeMeta, OutputTypeMeta,
};

#[derive(Clone, Copy)]
pub struct CapabilityRegistration {
    pub meta: &'static CapabilityMeta,
    pub input_type: &'static InputTypeMeta,
    #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
    pub executor: &'static CapabilityExecutor,
}

pub static CAPABILITY_REGISTRATIONS: &[CapabilityRegistration] = &[
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::sftp::__CAPABILITY_META_SFTP_LIST_FILES,
        input_type: &crate::sftp::__INPUT_META_SftpListFilesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::sftp::__CAPABILITY_EXECUTOR_SFTP_LIST_FILES,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::sftp::__CAPABILITY_META_SFTP_DOWNLOAD_FILE,
        input_type: &crate::sftp::__INPUT_META_SftpDownloadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::sftp::__CAPABILITY_EXECUTOR_SFTP_DOWNLOAD_FILE,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::sftp::__CAPABILITY_META_SFTP_UPLOAD_FILE,
        input_type: &crate::sftp::__INPUT_META_SftpUploadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::sftp::__CAPABILITY_EXECUTOR_SFTP_UPLOAD_FILE,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::sftp::__CAPABILITY_META_SFTP_DELETE_FILE,
        input_type: &crate::sftp::__INPUT_META_SftpDeleteFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::sftp::__CAPABILITY_EXECUTOR_SFTP_DELETE_FILE,
    },
];

pub static INPUT_TYPES: &[&InputTypeMeta] = &[
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpListFilesInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDownloadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpUploadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDeleteFileInput,
];

pub static OUTPUT_TYPES: &[&OutputTypeMeta] = &[
    &crate::types::__OUTPUT_META_FileData,
    &crate::types::__OUTPUT_META_LlmUsage,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_FileInfo,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_DeleteFileResponse,
];

pub static CONNECTION_TYPES: &[&ConnectionTypeMeta] = &[
    &crate::extractors::http_api_key::__CONNECTION_META_HttpApiKeyParams,
    &crate::extractors::http_bearer::__CONNECTION_META_HttpBearerParams,
    &crate::extractors::sftp::__CONNECTION_META_SftpParams,
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

/// Agent modules that exist beyond `runtara_dsl`'s `BUILTIN_AGENT_MODULES`.
///
/// Empty since compression and XLSX became WASM components: a component agent
/// carries its own module config in the `meta.json` sidecar emitted from
/// `agent_info()`, which the component dispatcher loads at boot. Kept as the
/// extension point rather than deleted — `get_all_agent_modules` still folds it
/// in, so a future host-only module has somewhere to register.
pub static EXTRA_AGENT_MODULES: &[&AgentModuleConfig] = &[];
