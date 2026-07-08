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
        meta: &crate::compression::__CAPABILITY_META_CREATE_ARCHIVE,
        input_type: &crate::compression::__INPUT_META_CreateArchiveInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::compression::__CAPABILITY_EXECUTOR_CREATE_ARCHIVE,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::compression::__CAPABILITY_META_EXTRACT_ARCHIVE,
        input_type: &crate::compression::__INPUT_META_ExtractArchiveInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::compression::__CAPABILITY_EXECUTOR_EXTRACT_ARCHIVE,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::compression::__CAPABILITY_META_EXTRACT_FILE,
        input_type: &crate::compression::__INPUT_META_ExtractFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::compression::__CAPABILITY_EXECUTOR_EXTRACT_FILE,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::compression::__CAPABILITY_META_LIST_ARCHIVE,
        input_type: &crate::compression::__INPUT_META_ListArchiveInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::compression::__CAPABILITY_EXECUTOR_LIST_ARCHIVE,
    },
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
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::xlsx::__CAPABILITY_META_FROM_XLSX,
        input_type: &crate::xlsx::__INPUT_META_FromXlsxInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::xlsx::__CAPABILITY_EXECUTOR_FROM_XLSX,
    },
    #[cfg(feature = "native")]
    CapabilityRegistration {
        meta: &crate::xlsx::__CAPABILITY_META_GET_SHEETS,
        input_type: &crate::xlsx::__INPUT_META_GetSheetsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::xlsx::__CAPABILITY_EXECUTOR_GET_SHEETS,
    },
];

pub static INPUT_TYPES: &[&InputTypeMeta] = &[
    #[cfg(feature = "native")]
    &crate::compression::__INPUT_META_ArchiveFileEntry,
    #[cfg(feature = "native")]
    &crate::compression::__INPUT_META_CreateArchiveInput,
    #[cfg(feature = "native")]
    &crate::compression::__INPUT_META_ExtractArchiveInput,
    #[cfg(feature = "native")]
    &crate::compression::__INPUT_META_ExtractFileInput,
    #[cfg(feature = "native")]
    &crate::compression::__INPUT_META_ListArchiveInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpListFilesInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDownloadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpUploadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDeleteFileInput,
    #[cfg(feature = "native")]
    &crate::xlsx::__INPUT_META_FromXlsxInput,
    #[cfg(feature = "native")]
    &crate::xlsx::__INPUT_META_GetSheetsInput,
];

pub static OUTPUT_TYPES: &[&OutputTypeMeta] = &[
    &crate::types::__OUTPUT_META_FileData,
    #[cfg(feature = "native")]
    &crate::compression::__OUTPUT_META_ExtractedFile,
    #[cfg(feature = "native")]
    &crate::compression::__OUTPUT_META_ExtractArchiveOutput,
    #[cfg(feature = "native")]
    &crate::compression::__OUTPUT_META_ArchiveEntryInfo,
    #[cfg(feature = "native")]
    &crate::compression::__OUTPUT_META_ListArchiveOutput,
    &crate::types::__OUTPUT_META_LlmUsage,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_FileInfo,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_DeleteFileResponse,
    #[cfg(feature = "native")]
    &crate::xlsx::__OUTPUT_META_SheetInfo,
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

#[cfg(feature = "native")]
const XLSX_AGENT_MODULE: AgentModuleConfig = AgentModuleConfig {
    id: "xlsx",
    name: "Spreadsheet",
    description: "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS)",
    has_side_effects: false,
    supports_connections: false,
    integration_ids: &[],
    secure: false,
};

pub static EXTRA_AGENT_MODULES: &[&AgentModuleConfig] = &[
    #[cfg(feature = "native")]
    &XLSX_AGENT_MODULE,
];
