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
    CapabilityRegistration {
        meta: &crate::crypto::__CAPABILITY_META_HASH,
        input_type: &crate::crypto::__INPUT_META_HashInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::crypto::__CAPABILITY_EXECUTOR_HASH,
    },
    CapabilityRegistration {
        meta: &crate::crypto::__CAPABILITY_META_HMAC,
        input_type: &crate::crypto::__INPUT_META_HmacInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::crypto::__CAPABILITY_EXECUTOR_HMAC,
    },
    CapabilityRegistration {
        meta: &crate::csv::__CAPABILITY_META_FROM_CSV,
        input_type: &crate::csv::__INPUT_META_FromCsvInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::csv::__CAPABILITY_EXECUTOR_FROM_CSV,
    },
    CapabilityRegistration {
        meta: &crate::csv::__CAPABILITY_META_TO_CSV,
        input_type: &crate::csv::__INPUT_META_ToCsvInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::csv::__CAPABILITY_EXECUTOR_TO_CSV,
    },
    CapabilityRegistration {
        meta: &crate::csv::__CAPABILITY_META_GET_HEADER,
        input_type: &crate::csv::__INPUT_META_GetHeaderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::csv::__CAPABILITY_EXECUTOR_GET_HEADER,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_GET_CURRENT_DATE,
        input_type: &crate::datetime::__INPUT_META_GetCurrentDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_GET_CURRENT_DATE,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_FORMAT_DATE,
        input_type: &crate::datetime::__INPUT_META_FormatDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_FORMAT_DATE,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_ADD_TO_DATE,
        input_type: &crate::datetime::__INPUT_META_AddToDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_ADD_TO_DATE,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_SUBTRACT_FROM_DATE,
        input_type: &crate::datetime::__INPUT_META_SubtractFromDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_SUBTRACT_FROM_DATE,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_GET_TIME_BETWEEN,
        input_type: &crate::datetime::__INPUT_META_GetTimeBetweenInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_GET_TIME_BETWEEN,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_EXTRACT_DATE_PART,
        input_type: &crate::datetime::__INPUT_META_ExtractDatePartInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_EXTRACT_DATE_PART,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_ROUND_DATE,
        input_type: &crate::datetime::__INPUT_META_RoundDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_ROUND_DATE,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_DATE_TO_UNIX,
        input_type: &crate::datetime::__INPUT_META_DateToUnixInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_DATE_TO_UNIX,
    },
    CapabilityRegistration {
        meta: &crate::datetime::__CAPABILITY_META_UNIX_TO_DATE,
        input_type: &crate::datetime::__INPUT_META_UnixToDateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::datetime::__CAPABILITY_EXECUTOR_UNIX_TO_DATE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_WRITE_FILE,
        input_type: &crate::file::__INPUT_META_WriteFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_WRITE_FILE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_READ_FILE,
        input_type: &crate::file::__INPUT_META_ReadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_READ_FILE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_LIST_FILES,
        input_type: &crate::file::__INPUT_META_ListFilesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_LIST_FILES,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_DELETE_FILE,
        input_type: &crate::file::__INPUT_META_DeleteFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_DELETE_FILE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_FILE_EXISTS,
        input_type: &crate::file::__INPUT_META_FileExistsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_FILE_EXISTS,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_COPY_FILE,
        input_type: &crate::file::__INPUT_META_CopyFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_COPY_FILE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_MOVE_FILE,
        input_type: &crate::file::__INPUT_META_MoveFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_MOVE_FILE,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_CREATE_DIRECTORY,
        input_type: &crate::file::__INPUT_META_CreateDirectoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_CREATE_DIRECTORY,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_GET_FILE_INFO,
        input_type: &crate::file::__INPUT_META_GetFileInfoInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_GET_FILE_INFO,
    },
    CapabilityRegistration {
        meta: &crate::file::__CAPABILITY_META_FILE_APPEND_FILE,
        input_type: &crate::file::__INPUT_META_AppendFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::file::__CAPABILITY_EXECUTOR_FILE_APPEND_FILE,
    },
    CapabilityRegistration {
        meta: &crate::http::__CAPABILITY_META_HTTP_REQUEST,
        input_type: &crate::http::__INPUT_META_HttpRequestInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::http::__CAPABILITY_EXECUTOR_HTTP_REQUEST,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::ai_tools::__CAPABILITY_META_AI_TEXT_COMPLETION,
        input_type: &crate::integrations::ai_tools::__INPUT_META_AiTextCompletionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_TEXT_COMPLETION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::ai_tools::__CAPABILITY_META_AI_IMAGE_GENERATION,
        input_type: &crate::integrations::ai_tools::__INPUT_META_AiImageGenerationInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_IMAGE_GENERATION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::ai_tools::__CAPABILITY_META_AI_VISION_TO_TEXT,
        input_type: &crate::integrations::ai_tools::__INPUT_META_AiVisionToTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_VISION_TO_TEXT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::ai_tools::__CAPABILITY_META_AI_VISION_TO_IMAGE,
        input_type: &crate::integrations::ai_tools::__INPUT_META_AiVisionToImageInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_VISION_TO_IMAGE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::ai_tools::__CAPABILITY_META_AI_EMBED_TEXT,
        input_type: &crate::integrations::ai_tools::__INPUT_META_AiEmbedTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_EMBED_TEXT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_TEXT_COMPLETION,
        input_type: &crate::integrations::bedrock::__INPUT_META_TextCompletionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_TEXT_COMPLETION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_IMAGE_GENERATION,
        input_type: &crate::integrations::bedrock::__INPUT_META_ImageGenerationInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_IMAGE_GENERATION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_STRUCTURED_OUTPUT,
        input_type: &crate::integrations::bedrock::__INPUT_META_StructuredOutputInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_STRUCTURED_OUTPUT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_VISION_TO_TEXT,
        input_type: &crate::integrations::bedrock::__INPUT_META_VisionToTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_VISION_TO_TEXT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_VISION_TO_IMAGE,
        input_type: &crate::integrations::bedrock::__INPUT_META_VisionToImageInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_VISION_TO_IMAGE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_BEDROCK_INVOKE_MODEL,
        input_type: &crate::integrations::bedrock::__INPUT_META_BedrockInvokeModelInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_BEDROCK_INVOKE_MODEL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::bedrock::__CAPABILITY_META_BEDROCK_LIST_MODELS,
        input_type: &crate::integrations::bedrock::__INPUT_META_BedrockListModelsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::bedrock::__CAPABILITY_EXECUTOR_BEDROCK_LIST_MODELS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_PRODUCTS,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_LOCATIONS,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetLocationsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_LOCATIONS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_PRODUCT,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_CREATE_PRODUCT,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceCreateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_CREATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_UPDATE_PRODUCT,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceUpdateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_DELETE_PRODUCT,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceDeleteProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_DELETE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_INVENTORY,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetInventoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_INVENTORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_UPDATE_INVENTORY,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceUpdateInventoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_UPDATE_INVENTORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_ORDERS,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetOrdersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_ORDERS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::commerce::__CAPABILITY_META_GET_ORDER,
        input_type: &crate::integrations::commerce::__INPUT_META_CommerceGetOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::commerce::__CAPABILITY_EXECUTOR_GET_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_CONTACTS,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListContactsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_CONTACTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_CONTACT,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetContactInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_CONTACT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_CONTACT,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateContactInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_CONTACT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_UPDATE_CONTACT,
        input_type: &crate::integrations::hubspot::__INPUT_META_UpdateContactInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_CONTACT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_DELETE_CONTACT,
        input_type: &crate::integrations::hubspot::__INPUT_META_DeleteContactInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_CONTACT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_SEARCH_CONTACTS,
        input_type: &crate::integrations::hubspot::__INPUT_META_SearchContactsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_CONTACTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_COMPANIES,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListCompaniesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_COMPANIES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_COMPANY,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetCompanyInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_COMPANY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_COMPANY,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateCompanyInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_COMPANY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_UPDATE_COMPANY,
        input_type: &crate::integrations::hubspot::__INPUT_META_UpdateCompanyInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_COMPANY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_DELETE_COMPANY,
        input_type: &crate::integrations::hubspot::__INPUT_META_DeleteCompanyInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_COMPANY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_SEARCH_COMPANIES,
        input_type: &crate::integrations::hubspot::__INPUT_META_SearchCompaniesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_COMPANIES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_DEALS,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListDealsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_DEALS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_DEAL,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetDealInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_DEAL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_DEAL,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateDealInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_DEAL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_UPDATE_DEAL,
        input_type: &crate::integrations::hubspot::__INPUT_META_UpdateDealInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_DEAL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_DELETE_DEAL,
        input_type: &crate::integrations::hubspot::__INPUT_META_DeleteDealInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_DEAL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_SEARCH_DEALS,
        input_type: &crate::integrations::hubspot::__INPUT_META_SearchDealsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_DEALS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_QUOTES,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListQuotesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_QUOTES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_QUOTE,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetQuoteInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_QUOTE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_QUOTE,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateQuoteInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_QUOTE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_UPDATE_QUOTE,
        input_type: &crate::integrations::hubspot::__INPUT_META_UpdateQuoteInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_QUOTE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_LINE_ITEMS,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListLineItemsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_LINE_ITEMS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_LINE_ITEM,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateLineItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_LINE_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_DELETE_LINE_ITEM,
        input_type: &crate::integrations::hubspot::__INPUT_META_DeleteLineItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_LINE_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_OWNERS,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListOwnersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_OWNERS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_OWNER,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetOwnerInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_OWNER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_PIPELINES,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListPipelinesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_PIPELINES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_GET_PIPELINE,
        input_type: &crate::integrations::hubspot::__INPUT_META_GetPipelineInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_PIPELINE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_CREATE_ASSOCIATION,
        input_type: &crate::integrations::hubspot::__INPUT_META_CreateAssociationInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_ASSOCIATION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::hubspot::__CAPABILITY_META_LIST_ASSOCIATIONS,
        input_type: &crate::integrations::hubspot::__INPUT_META_ListAssociationsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_ASSOCIATIONS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::mailgun::__CAPABILITY_META_SEND_EMAIL,
        input_type: &crate::integrations::mailgun::__INPUT_META_SendEmailInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::mailgun::__CAPABILITY_EXECUTOR_SEND_EMAIL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_CREATE_INSTANCE,
        input_type: &crate::integrations::object_model::__INPUT_META_CreateInstanceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_CREATE_INSTANCE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_QUERY_INSTANCES,
        input_type: &crate::integrations::object_model::__INPUT_META_QueryInstancesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_QUERY_INSTANCES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_CHECK_INSTANCE_EXISTS,
        input_type: &crate::integrations::object_model::__INPUT_META_CheckInstanceExistsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_CHECK_INSTANCE_EXISTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_CREATE_IF_NOT_EXISTS,
        input_type: &crate::integrations::object_model::__INPUT_META_CreateIfNotExistsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_CREATE_IF_NOT_EXISTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_UPDATE_INSTANCE,
        input_type: &crate::integrations::object_model::__INPUT_META_UpdateInstanceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_UPDATE_INSTANCE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_DELETE_INSTANCE,
        input_type: &crate::integrations::object_model::__INPUT_META_DeleteInstanceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_DELETE_INSTANCE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_BULK_CREATE_INSTANCES,
        input_type: &crate::integrations::object_model::__INPUT_META_BulkCreateInstancesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_CREATE_INSTANCES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_BULK_UPDATE_INSTANCES,
        input_type: &crate::integrations::object_model::__INPUT_META_BulkUpdateInstancesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_UPDATE_INSTANCES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_BULK_DELETE_INSTANCES,
        input_type: &crate::integrations::object_model::__INPUT_META_BulkDeleteInstancesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_DELETE_INSTANCES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_QUERY_AGGREGATE,
        input_type: &crate::integrations::object_model::__INPUT_META_QueryAggregateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_QUERY_AGGREGATE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_LOAD_MEMORY,
        input_type: &crate::integrations::object_model::__INPUT_META_LoadMemoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_LOAD_MEMORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::object_model::__CAPABILITY_META_SAVE_MEMORY,
        input_type: &crate::integrations::object_model::__INPUT_META_SaveMemoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::object_model::__CAPABILITY_EXECUTOR_SAVE_MEMORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_TEXT_COMPLETION,
        input_type: &crate::integrations::openai::__INPUT_META_TextCompletionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_TEXT_COMPLETION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_IMAGE_GENERATION,
        input_type: &crate::integrations::openai::__INPUT_META_ImageGenerationInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_IMAGE_GENERATION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_STRUCTURED_OUTPUT,
        input_type: &crate::integrations::openai::__INPUT_META_StructuredOutputInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_STRUCTURED_OUTPUT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_VISION_TO_TEXT,
        input_type: &crate::integrations::openai::__INPUT_META_VisionToTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_VISION_TO_TEXT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_VISION_TO_IMAGE,
        input_type: &crate::integrations::openai::__INPUT_META_VisionToImageInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_VISION_TO_IMAGE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_OPENAI_CHAT_COMPLETION,
        input_type: &crate::integrations::openai::__INPUT_META_OpenaiChatCompletionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_CHAT_COMPLETION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_OPENAI_CREATE_EMBEDDING,
        input_type: &crate::integrations::openai::__INPUT_META_OpenaiCreateEmbeddingInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_CREATE_EMBEDDING,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::openai::__CAPABILITY_META_OPENAI_MODERATE_CONTENT,
        input_type: &crate::integrations::openai::__INPUT_META_OpenaiModerateContentInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_MODERATE_CONTENT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_CREATE_BUCKET,
        input_type: &crate::integrations::s3_storage::__INPUT_META_CreateBucketInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_CREATE_BUCKET,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_LIST_BUCKETS,
        input_type: &crate::integrations::s3_storage::__INPUT_META_ListBucketsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_LIST_BUCKETS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_DELETE_BUCKET,
        input_type: &crate::integrations::s3_storage::__INPUT_META_DeleteBucketInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DELETE_BUCKET,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_UPLOAD_FILE,
        input_type: &crate::integrations::s3_storage::__INPUT_META_UploadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_UPLOAD_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_DOWNLOAD_FILE,
        input_type: &crate::integrations::s3_storage::__INPUT_META_DownloadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DOWNLOAD_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_LIST_FILES,
        input_type: &crate::integrations::s3_storage::__INPUT_META_ListFilesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_LIST_FILES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_GET_FILE_INFO,
        input_type: &crate::integrations::s3_storage::__INPUT_META_GetFileInfoInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_GET_FILE_INFO,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_DELETE_FILE,
        input_type: &crate::integrations::s3_storage::__INPUT_META_DeleteFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DELETE_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::s3_storage::__CAPABILITY_META_STORAGE_COPY_FILE,
        input_type: &crate::integrations::s3_storage::__INPUT_META_CopyFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_COPY_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_LIST_DRIVES,
        input_type: &crate::integrations::sharepoint::__INPUT_META_ListDrivesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_LIST_DRIVES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_LIST_CHILDREN,
        input_type: &crate::integrations::sharepoint::__INPUT_META_ListChildrenInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_LIST_CHILDREN,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_GET_ITEM,
        input_type: &crate::integrations::sharepoint::__INPUT_META_GetItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_GET_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_GET_ITEM_BY_PATH,
        input_type: &crate::integrations::sharepoint::__INPUT_META_GetItemByPathInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor:
            &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_GET_ITEM_BY_PATH,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_DOWNLOAD_FILE,
        input_type: &crate::integrations::sharepoint::__INPUT_META_DownloadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_DOWNLOAD_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_UPLOAD_FILE,
        input_type: &crate::integrations::sharepoint::__INPUT_META_UploadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_UPLOAD_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_UPLOAD_FILE_LARGE,
        input_type: &crate::integrations::sharepoint::__INPUT_META_UploadFileLargeInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor:
            &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_UPLOAD_FILE_LARGE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_CREATE_FOLDER,
        input_type: &crate::integrations::sharepoint::__INPUT_META_CreateFolderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_CREATE_FOLDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_DELETE_ITEM,
        input_type: &crate::integrations::sharepoint::__INPUT_META_DeleteItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_DELETE_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_MOVE_ITEM,
        input_type: &crate::integrations::sharepoint::__INPUT_META_MoveItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_MOVE_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_COPY_ITEM,
        input_type: &crate::integrations::sharepoint::__INPUT_META_CopyItemInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_COPY_ITEM,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_GET_COPY_STATUS,
        input_type: &crate::integrations::sharepoint::__INPUT_META_GetCopyStatusInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor:
            &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_GET_COPY_STATUS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_SEARCH,
        input_type: &crate::integrations::sharepoint::__INPUT_META_SearchInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_SEARCH,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::sharepoint::__CAPABILITY_META_SHAREPOINT_SEARCH_GLOBAL,
        input_type: &crate::integrations::sharepoint::__INPUT_META_SearchGlobalInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::sharepoint::__CAPABILITY_EXECUTOR_SHAREPOINT_SEARCH_GLOBAL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_SetProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_UPDATE_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_UpdateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_DELETE_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_DeleteProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_DELETE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_LIST_PRODUCTS,
        input_type: &crate::integrations::shopify::__INPUT_META_ListProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_LIST_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_QUERY_PRODUCTS,
        input_type: &crate::integrations::shopify::__INPUT_META_QueryProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_QUERY_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_PRODUCT_BY_SKU,
        input_type: &crate::integrations::shopify::__INPUT_META_GetProductBySkuInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_BY_SKU,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_PRODUCT_TAGS,
        input_type: &crate::integrations::shopify::__INPUT_META_SetProductTagsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_TAGS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_REPLACE_PRODUCT_IMAGES,
        input_type: &crate::integrations::shopify::__INPUT_META_ReplaceProductImagesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_REPLACE_PRODUCT_IMAGES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_PRODUCT_OPTIONS,
        input_type: &crate::integrations::shopify::__INPUT_META_GetProductOptionsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_OPTIONS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_RENAME_PRODUCT_OPTION,
        input_type: &crate::integrations::shopify::__INPUT_META_RenameProductOptionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_RENAME_PRODUCT_OPTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_PRODUCT_METAFIELDS,
        input_type: &crate::integrations::shopify::__INPUT_META_SetProductMetafieldsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_METAFIELDS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_PRODUCT_METAFIELDS,
        input_type: &crate::integrations::shopify::__INPUT_META_GetProductMetafieldsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_METAFIELDS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_PRODUCT_VARIANT_BY_SKU,
        input_type: &crate::integrations::shopify::__INPUT_META_GetProductVariantBySkuInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_VARIANT_BY_SKU,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_CREATE_PRODUCT_VARIANT,
        input_type: &crate::integrations::shopify::__INPUT_META_CreateProductVariantInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_PRODUCT_VARIANT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_UPDATE_PRODUCT_VARIANT,
        input_type: &crate::integrations::shopify::__INPUT_META_UpdateProductVariantInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT_VARIANT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_UPDATE_PRODUCT_VARIANT_PRICE,
        input_type: &crate::integrations::shopify::__INPUT_META_UpdateProductVariantPriceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT_VARIANT_PRICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_DELETE_PRODUCT_VARIANT,
        input_type: &crate::integrations::shopify::__INPUT_META_DeleteProductVariantInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_DELETE_PRODUCT_VARIANT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_VARIANT_METAFIELDS,
        input_type: &crate::integrations::shopify::__INPUT_META_SetVariantMetafieldsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_VARIANT_METAFIELDS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_PRODUCT_VARIANT_COST,
        input_type: &crate::integrations::shopify::__INPUT_META_SetProductVariantCostInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_VARIANT_COST,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_PRODUCT_VARIANT_WEIGHT,
        input_type: &crate::integrations::shopify::__INPUT_META_SetProductVariantWeightInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_VARIANT_WEIGHT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_INVENTORY_ITEM_ID_BY_VARIANT_ID,
        input_type: &crate::integrations::shopify::__INPUT_META_GetInventoryItemIdByVariantIdInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor:
            &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_INVENTORY_ITEM_ID_BY_VARIANT_ID,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SET_INVENTORY,
        input_type: &crate::integrations::shopify::__INPUT_META_SetInventoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SET_INVENTORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_SYNC_INVENTORY_LEVELS,
        input_type: &crate::integrations::shopify::__INPUT_META_SyncInventoryLevelsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_SYNC_INVENTORY_LEVELS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_ORDER,
        input_type: &crate::integrations::shopify::__INPUT_META_GetOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_ORDER_LIST,
        input_type: &crate::integrations::shopify::__INPUT_META_GetOrderListInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_ORDER_LIST,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_CREATE_ORDER_NOTE_OR_TAG,
        input_type: &crate::integrations::shopify::__INPUT_META_CreateOrderNoteOrTagInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_ORDER_NOTE_OR_TAG,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_CANCEL_ORDER,
        input_type: &crate::integrations::shopify::__INPUT_META_CancelOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_CANCEL_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_FULFILLMENT_ORDERS,
        input_type: &crate::integrations::shopify::__INPUT_META_GetFulfillmentOrdersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_FULFILLMENT_ORDERS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_FULFILL_ORDER,
        input_type: &crate::integrations::shopify::__INPUT_META_FulfillOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_FULFILL_ORDER_LINES,
        input_type: &crate::integrations::shopify::__INPUT_META_FulfillOrderLinesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_ORDER_LINES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_FULFILL_BY_SKU,
        input_type: &crate::integrations::shopify::__INPUT_META_FulfillBySkuInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_BY_SKU,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_CREATE_DRAFT_ORDER,
        input_type: &crate::integrations::shopify::__INPUT_META_CreateDraftOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_DRAFT_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_CUSTOMER_BY_EMAIL,
        input_type: &crate::integrations::shopify::__INPUT_META_GetCustomerByEmailInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_CUSTOMER_BY_EMAIL,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_CREATE_COLLECTION,
        input_type: &crate::integrations::shopify::__INPUT_META_CreateCollectionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_COLLECTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_ADD_PRODUCTS_TO_COLLECTION,
        input_type: &crate::integrations::shopify::__INPUT_META_AddProductsToCollectionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_ADD_PRODUCTS_TO_COLLECTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_REMOVE_PRODUCTS_FROM_COLLECTION,
        input_type: &crate::integrations::shopify::__INPUT_META_RemoveProductsFromCollectionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor:
            &crate::integrations::shopify::__CAPABILITY_EXECUTOR_REMOVE_PRODUCTS_FROM_COLLECTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_GET_LOCATION_BY_NAME,
        input_type: &crate::integrations::shopify::__INPUT_META_GetLocationByNameInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_GET_LOCATION_BY_NAME,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_BULK_CREATE_PRODUCTS,
        input_type: &crate::integrations::shopify::__INPUT_META_BulkCreateProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_CREATE_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_BULK_UPDATE_PRODUCTS,
        input_type: &crate::integrations::shopify::__INPUT_META_BulkUpdateProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_UPDATE_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_BULK_UPDATE_VARIANT_PRICES,
        input_type: &crate::integrations::shopify::__INPUT_META_BulkUpdateVariantPricesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_UPDATE_VARIANT_PRICES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_PRODUCTS,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_CREATE_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceCreateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_CREATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_UPDATE_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceUpdateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_UPDATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_DELETE_PRODUCT,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceDeleteProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_DELETE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_INVENTORY,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetInventoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_INVENTORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_UPDATE_INVENTORY,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceUpdateInventoryInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_UPDATE_INVENTORY,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_ORDERS,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetOrdersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_ORDERS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_ORDER,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetOrderInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_ORDER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::shopify::__CAPABILITY_META_COMMERCE_GET_LOCATIONS,
        input_type: &crate::integrations::shopify::__INPUT_META_CommerceGetLocationsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_LOCATIONS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::slack::__CAPABILITY_META_SEND_MESSAGE,
        input_type: &crate::integrations::slack::__INPUT_META_SendMessageInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::slack::__CAPABILITY_EXECUTOR_SEND_MESSAGE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::slack::__CAPABILITY_META_UPLOAD_FILE,
        input_type: &crate::integrations::slack::__INPUT_META_UploadFileInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::slack::__CAPABILITY_EXECUTOR_UPLOAD_FILE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_CUSTOMERS,
        input_type: &crate::integrations::stripe::__INPUT_META_ListCustomersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_CUSTOMERS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_CUSTOMER,
        input_type: &crate::integrations::stripe::__INPUT_META_GetCustomerInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_CUSTOMER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_CUSTOMER,
        input_type: &crate::integrations::stripe::__INPUT_META_CreateCustomerInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_CUSTOMER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_UPDATE_CUSTOMER,
        input_type: &crate::integrations::stripe::__INPUT_META_UpdateCustomerInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_UPDATE_CUSTOMER,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_PRODUCTS,
        input_type: &crate::integrations::stripe::__INPUT_META_ListProductsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PRODUCTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_PRODUCT,
        input_type: &crate::integrations::stripe::__INPUT_META_GetProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_PRODUCT,
        input_type: &crate::integrations::stripe::__INPUT_META_CreateProductInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PRODUCT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_PRICES,
        input_type: &crate::integrations::stripe::__INPUT_META_ListPricesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PRICES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_PRICE,
        input_type: &crate::integrations::stripe::__INPUT_META_CreatePriceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PRICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_PAYMENT_INTENT,
        input_type: &crate::integrations::stripe::__INPUT_META_CreatePaymentIntentInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PAYMENT_INTENT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_PAYMENT_INTENT,
        input_type: &crate::integrations::stripe::__INPUT_META_GetPaymentIntentInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_PAYMENT_INTENT,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_PAYMENT_INTENTS,
        input_type: &crate::integrations::stripe::__INPUT_META_ListPaymentIntentsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PAYMENT_INTENTS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_INVOICE,
        input_type: &crate::integrations::stripe::__INPUT_META_CreateInvoiceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_INVOICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_INVOICE,
        input_type: &crate::integrations::stripe::__INPUT_META_GetInvoiceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_INVOICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_INVOICES,
        input_type: &crate::integrations::stripe::__INPUT_META_ListInvoicesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_INVOICES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_FINALIZE_INVOICE,
        input_type: &crate::integrations::stripe::__INPUT_META_FinalizeInvoiceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_FINALIZE_INVOICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_SEND_INVOICE,
        input_type: &crate::integrations::stripe::__INPUT_META_SendInvoiceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_SEND_INVOICE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_SUBSCRIPTION,
        input_type: &crate::integrations::stripe::__INPUT_META_CreateSubscriptionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_SUBSCRIPTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_SUBSCRIPTION,
        input_type: &crate::integrations::stripe::__INPUT_META_GetSubscriptionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_SUBSCRIPTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_SUBSCRIPTIONS,
        input_type: &crate::integrations::stripe::__INPUT_META_ListSubscriptionsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_SUBSCRIPTIONS,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CANCEL_SUBSCRIPTION,
        input_type: &crate::integrations::stripe::__INPUT_META_CancelSubscriptionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CANCEL_SUBSCRIPTION,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_CREATE_REFUND,
        input_type: &crate::integrations::stripe::__INPUT_META_CreateRefundInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_REFUND,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_REFUND,
        input_type: &crate::integrations::stripe::__INPUT_META_GetRefundInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_REFUND,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_BALANCE,
        input_type: &crate::integrations::stripe::__INPUT_META_GetBalanceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_BALANCE,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_LIST_CHARGES,
        input_type: &crate::integrations::stripe::__INPUT_META_ListChargesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_CHARGES,
    },
    #[cfg(feature = "integrations")]
    CapabilityRegistration {
        meta: &crate::integrations::stripe::__CAPABILITY_META_GET_CHARGE,
        input_type: &crate::integrations::stripe::__INPUT_META_GetChargeInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::integrations::stripe::__CAPABILITY_EXECUTOR_GET_CHARGE,
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
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_RENDER_TEMPLATE,
        input_type: &crate::text::__INPUT_META_TemplateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_RENDER_TEMPLATE,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_TRIM_NORMALIZE,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_TRIM_NORMALIZE,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_CASE_CONVERSION,
        input_type: &crate::text::__INPUT_META_CaseConversionInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_CASE_CONVERSION,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_FIND_REPLACE,
        input_type: &crate::text::__INPUT_META_FindReplaceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_FIND_REPLACE,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_EXTRACT_FIRST_LINE,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_EXTRACT_FIRST_LINE,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_EXTRACT_FIRST_WORD,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_EXTRACT_FIRST_WORD,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_SPLIT_JOIN,
        input_type: &crate::text::__INPUT_META_SplitInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_SPLIT_JOIN,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_SPLIT,
        input_type: &crate::text::__INPUT_META_SplitInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_SPLIT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_REMOVE_CHARACTERS,
        input_type: &crate::text::__INPUT_META_RemoveCharactersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_REMOVE_CHARACTERS,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_SUBSTRING_EXTRACTION,
        input_type: &crate::text::__INPUT_META_SubstringInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_SUBSTRING_EXTRACTION,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_COLLAPSE_EXPAND_LINES,
        input_type: &crate::text::__INPUT_META_SplitInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_COLLAPSE_EXPAND_LINES,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_SLUGIFY,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_SLUGIFY,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_HASH_TEXT,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_HASH_TEXT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_AS_BYTE_ARRAY,
        input_type: &crate::text::__INPUT_META_ByteArrayInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_AS_BYTE_ARRAY,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_FROM_BASE64,
        input_type: &crate::text::__INPUT_META_FromBase64Input,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_FROM_BASE64,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_TO_BASE64,
        input_type: &crate::text::__INPUT_META_ToBase64Input,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_TO_BASE64,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_REGEX_REPLACE,
        input_type: &crate::text::__INPUT_META_RegexReplaceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_REGEX_REPLACE,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_REGEX_MATCH,
        input_type: &crate::text::__INPUT_META_RegexMatchInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_REGEX_MATCH,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_REGEX_TEST,
        input_type: &crate::text::__INPUT_META_RegexTestInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_REGEX_TEST,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_REGEX_SPLIT,
        input_type: &crate::text::__INPUT_META_RegexSplitInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_REGEX_SPLIT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_PAD_TEXT,
        input_type: &crate::text::__INPUT_META_PadTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_PAD_TEXT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_TRUNCATE_TEXT,
        input_type: &crate::text::__INPUT_META_TruncateTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_TRUNCATE_TEXT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_WRAP_TEXT,
        input_type: &crate::text::__INPUT_META_WrapTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_WRAP_TEXT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_EXTRACT_NUMBERS,
        input_type: &crate::text::__INPUT_META_ExtractNumbersInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_EXTRACT_NUMBERS,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_EXTRACT_EMAILS,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_EXTRACT_EMAILS,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_EXTRACT_URLS,
        input_type: &crate::text::__INPUT_META_SimpleTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_EXTRACT_URLS,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_COMPARE_TEXT,
        input_type: &crate::text::__INPUT_META_CompareTextInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_COMPARE_TEXT,
    },
    CapabilityRegistration {
        meta: &crate::text::__CAPABILITY_META_COUNT_OCCURRENCES,
        input_type: &crate::text::__INPUT_META_CountOccurrencesInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::text::__CAPABILITY_EXECUTOR_COUNT_OCCURRENCES,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_EXTRACT,
        input_type: &crate::transform::__INPUT_META_ExtractInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_EXTRACT,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_GET_VALUE_BY_PATH,
        input_type: &crate::transform::__INPUT_META_GetValueByPathInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_GET_VALUE_BY_PATH,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_SET_VALUE_BY_PATH,
        input_type: &crate::transform::__INPUT_META_SetValueByPathInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_SET_VALUE_BY_PATH,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_FILTER_NON_VALUES,
        input_type: &crate::transform::__INPUT_META_FilterNoValueInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_FILTER_NON_VALUES,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_SELECT_FIRST,
        input_type: &crate::transform::__INPUT_META_SelectFirstInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_SELECT_FIRST,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_COALESCE,
        input_type: &crate::transform::__INPUT_META_CoalesceInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_COALESCE,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_FROM_JSON_STRING,
        input_type: &crate::transform::__INPUT_META_FromJsonStringInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_FROM_JSON_STRING,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_TO_JSON_STRING,
        input_type: &crate::transform::__INPUT_META_ToJsonStringInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_TO_JSON_STRING,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_FILTER,
        input_type: &crate::transform::__INPUT_META_FilterInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_FILTER,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_SORT,
        input_type: &crate::transform::__INPUT_META_SortInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_SORT,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_MAP_FIELDS,
        input_type: &crate::transform::__INPUT_META_MapFieldsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_MAP_FIELDS,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_GROUP_BY,
        input_type: &crate::transform::__INPUT_META_GroupByInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_GROUP_BY,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_APPEND,
        input_type: &crate::transform::__INPUT_META_AppendInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_APPEND,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_FLAT_MAP,
        input_type: &crate::transform::__INPUT_META_FlatMapInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_FLAT_MAP,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_ARRAY_LENGTH,
        input_type: &crate::transform::__INPUT_META_ArrayLengthInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_ARRAY_LENGTH,
    },
    CapabilityRegistration {
        meta: &crate::transform::__CAPABILITY_META_ENSURE_ARRAY,
        input_type: &crate::transform::__INPUT_META_EnsureArrayInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::transform::__CAPABILITY_EXECUTOR_ENSURE_ARRAY,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_RANDOM_DOUBLE,
        input_type: &crate::utils::__INPUT_META_RandomDoubleInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_RANDOM_DOUBLE,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_RANDOM_ARRAY,
        input_type: &crate::utils::__INPUT_META_ReturnRandomArrayInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_RANDOM_ARRAY,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_RETURN_INPUT_STRING,
        input_type: &crate::utils::__INPUT_META_ReturnStringInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_RETURN_INPUT_STRING,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_RETURN_INPUT,
        input_type: &crate::utils::__INPUT_META_ReturnInputData,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_RETURN_INPUT,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_DO_NOTHING,
        input_type: &crate::utils::__INPUT_META_DoNothingInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_DO_NOTHING,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_DELAY_IN_MS,
        input_type: &crate::utils::__INPUT_META_DelayInMsInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_DELAY_IN_MS,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_CALCULATE,
        input_type: &crate::utils::__INPUT_META_CalculateInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_CALCULATE,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_FORMAT_DATE_FROM_ISO,
        input_type: &crate::utils::__INPUT_META_FormatDateFromIsoInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_FORMAT_DATE_FROM_ISO,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_ISO_TO_UNIX_TIMESTAMP,
        input_type: &crate::utils::__INPUT_META_IsoToUnixTimestampInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_ISO_TO_UNIX_TIMESTAMP,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_GET_CURRENT_UNIX_TIMESTAMP,
        input_type: &crate::utils::__INPUT_META_GetCurrentUnixTimestampInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_UNIX_TIMESTAMP,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_GET_CURRENT_ISO_DATETIME,
        input_type: &crate::utils::__INPUT_META_GetCurrentIsoDatetimeInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_ISO_DATETIME,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_GET_CURRENT_FORMATTED_DATETIME,
        input_type: &crate::utils::__INPUT_META_GetCurrentFormattedDateTimeInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_FORMATTED_DATETIME,
    },
    CapabilityRegistration {
        meta: &crate::utils::__CAPABILITY_META_COUNTRY_NAME_TO_ISO_CODE,
        input_type: &crate::utils::__INPUT_META_CountryNameToIsoCodeInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::utils::__CAPABILITY_EXECUTOR_COUNTRY_NAME_TO_ISO_CODE,
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
    CapabilityRegistration {
        meta: &crate::xml::__CAPABILITY_META_FROM_XML,
        input_type: &crate::xml::__INPUT_META_FromXmlInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::xml::__CAPABILITY_EXECUTOR_FROM_XML,
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
    &crate::crypto::__INPUT_META_HashInput,
    &crate::crypto::__INPUT_META_HmacInput,
    &crate::csv::__INPUT_META_FromCsvInput,
    &crate::csv::__INPUT_META_ToCsvInput,
    &crate::csv::__INPUT_META_GetHeaderInput,
    &crate::datetime::__INPUT_META_GetCurrentDateInput,
    &crate::datetime::__INPUT_META_FormatDateInput,
    &crate::datetime::__INPUT_META_AddToDateInput,
    &crate::datetime::__INPUT_META_SubtractFromDateInput,
    &crate::datetime::__INPUT_META_GetTimeBetweenInput,
    &crate::datetime::__INPUT_META_ExtractDatePartInput,
    &crate::datetime::__INPUT_META_DateToUnixInput,
    &crate::datetime::__INPUT_META_UnixToDateInput,
    &crate::datetime::__INPUT_META_RoundDateInput,
    &crate::file::__INPUT_META_WriteFileInput,
    &crate::file::__INPUT_META_ReadFileInput,
    &crate::file::__INPUT_META_ListFilesInput,
    &crate::file::__INPUT_META_DeleteFileInput,
    &crate::file::__INPUT_META_FileExistsInput,
    &crate::file::__INPUT_META_CopyFileInput,
    &crate::file::__INPUT_META_MoveFileInput,
    &crate::file::__INPUT_META_CreateDirectoryInput,
    &crate::file::__INPUT_META_GetFileInfoInput,
    &crate::file::__INPUT_META_AppendFileInput,
    &crate::http::__INPUT_META_HttpRequestInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__INPUT_META_AiTextCompletionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__INPUT_META_AiImageGenerationInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__INPUT_META_AiVisionToTextInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__INPUT_META_AiVisionToImageInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__INPUT_META_AiEmbedTextInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_TextCompletionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_ImageGenerationInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_StructuredOutputInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_VisionToTextInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_VisionToImageInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_BedrockInvokeModelInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__INPUT_META_BedrockListModelsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetLocationsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceCreateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceUpdateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceDeleteProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetInventoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceUpdateInventoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetOrdersInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__INPUT_META_CommerceGetOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListContactsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetContactInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateContactInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_UpdateContactInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_DeleteContactInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_SearchContactsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListCompaniesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetCompanyInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateCompanyInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_UpdateCompanyInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_DeleteCompanyInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_SearchCompaniesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListDealsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetDealInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateDealInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_UpdateDealInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_DeleteDealInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_SearchDealsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListQuotesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetQuoteInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateQuoteInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_UpdateQuoteInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListLineItemsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateLineItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_DeleteLineItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListOwnersInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetOwnerInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListPipelinesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_GetPipelineInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_CreateAssociationInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__INPUT_META_ListAssociationsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::mailgun::__INPUT_META_SendEmailInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_CreateInstanceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_QueryInstancesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_CheckInstanceExistsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_CreateIfNotExistsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_UpdateInstanceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_DeleteInstanceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_BulkCreateInstancesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_BulkUpdateInstancesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_BulkDeleteInstancesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_QueryAggregateInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_LoadMemoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__INPUT_META_SaveMemoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_TextCompletionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_ImageGenerationInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_StructuredOutputInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_VisionToTextInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_VisionToImageInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_OpenaiChatCompletionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_OpenaiCreateEmbeddingInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__INPUT_META_OpenaiModerateContentInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_CreateBucketInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_ListBucketsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_DeleteBucketInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_UploadFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_DownloadFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_ListFilesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_GetFileInfoInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_DeleteFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__INPUT_META_CopyFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_ListDrivesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_ListChildrenInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_GetItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_GetItemByPathInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_DownloadFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_UploadFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_UploadFileLargeInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_CreateFolderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_DeleteItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_MoveItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_CopyItemInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_GetCopyStatusInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_SearchInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__INPUT_META_SearchGlobalInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_UpdateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_DeleteProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_ListProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_QueryProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetProductBySkuInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetProductTagsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_ReplaceProductImagesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetProductOptionsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_RenameProductOptionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetProductMetafieldsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetProductMetafieldsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetProductVariantBySkuInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CreateProductVariantInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_UpdateProductVariantInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_UpdateProductVariantPriceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_DeleteProductVariantInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetVariantMetafieldsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetProductVariantCostInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetProductVariantWeightInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetInventoryItemIdByVariantIdInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SetInventoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_SyncInventoryLevelsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetOrderListInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CreateOrderNoteOrTagInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CancelOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetFulfillmentOrdersInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_FulfillOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_FulfillOrderLinesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_FulfillBySkuInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CreateDraftOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetCustomerByEmailInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CreateCollectionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_AddProductsToCollectionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_RemoveProductsFromCollectionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_GetLocationByNameInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_BulkCreateProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_BulkUpdateProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_BulkUpdateVariantPricesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceCreateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceUpdateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceDeleteProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetInventoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceUpdateInventoryInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetOrdersInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetOrderInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__INPUT_META_CommerceGetLocationsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::slack::__INPUT_META_SendMessageInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::slack::__INPUT_META_UploadFileInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListCustomersInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetCustomerInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreateCustomerInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_UpdateCustomerInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListProductsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreateProductInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListPricesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreatePriceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreatePaymentIntentInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetPaymentIntentInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListPaymentIntentsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreateInvoiceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetInvoiceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListInvoicesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_FinalizeInvoiceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_SendInvoiceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreateSubscriptionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetSubscriptionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListSubscriptionsInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CancelSubscriptionInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_CreateRefundInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetRefundInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetBalanceInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_ListChargesInput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__INPUT_META_GetChargeInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpListFilesInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDownloadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpUploadFileInput,
    #[cfg(feature = "native")]
    &crate::sftp::__INPUT_META_SftpDeleteFileInput,
    &crate::text::__INPUT_META_SimpleTextInput,
    &crate::text::__INPUT_META_TemplateInput,
    &crate::text::__INPUT_META_CaseConversionInput,
    &crate::text::__INPUT_META_FindReplaceInput,
    &crate::text::__INPUT_META_RemoveCharactersInput,
    &crate::text::__INPUT_META_SplitInput,
    &crate::text::__INPUT_META_SubstringInput,
    &crate::text::__INPUT_META_ByteArrayInput,
    &crate::text::__INPUT_META_FromBase64Input,
    &crate::text::__INPUT_META_ToBase64Input,
    &crate::text::__INPUT_META_RegexReplaceInput,
    &crate::text::__INPUT_META_RegexMatchInput,
    &crate::text::__INPUT_META_RegexTestInput,
    &crate::text::__INPUT_META_RegexSplitInput,
    &crate::text::__INPUT_META_PadTextInput,
    &crate::text::__INPUT_META_TruncateTextInput,
    &crate::text::__INPUT_META_WrapTextInput,
    &crate::text::__INPUT_META_ExtractNumbersInput,
    &crate::text::__INPUT_META_CompareTextInput,
    &crate::text::__INPUT_META_CountOccurrencesInput,
    &crate::transform::__INPUT_META_ExtractInput,
    &crate::transform::__INPUT_META_GetValueByPathInput,
    &crate::transform::__INPUT_META_SetValueByPathInput,
    &crate::transform::__INPUT_META_FilterNoValueInput,
    &crate::transform::__INPUT_META_SelectFirstInput,
    &crate::transform::__INPUT_META_CoalesceInput,
    &crate::transform::__INPUT_META_FromJsonStringInput,
    &crate::transform::__INPUT_META_ToJsonStringInput,
    &crate::transform::__INPUT_META_FilterInput,
    &crate::transform::__INPUT_META_SortInput,
    &crate::transform::__INPUT_META_MapFieldsInput,
    &crate::transform::__INPUT_META_GroupByInput,
    &crate::transform::__INPUT_META_AppendInput,
    &crate::transform::__INPUT_META_FlatMapInput,
    &crate::transform::__INPUT_META_ArrayLengthInput,
    &crate::transform::__INPUT_META_EnsureArrayInput,
    &crate::utils::__INPUT_META_RandomDoubleInput,
    &crate::utils::__INPUT_META_ReturnRandomArrayInput,
    &crate::utils::__INPUT_META_ReturnStringInput,
    &crate::utils::__INPUT_META_ReturnInputData,
    &crate::utils::__INPUT_META_DoNothingInput,
    &crate::utils::__INPUT_META_DelayInMsInput,
    &crate::utils::__INPUT_META_CalculateInput,
    &crate::utils::__INPUT_META_FormatDateFromIsoInput,
    &crate::utils::__INPUT_META_IsoToUnixTimestampInput,
    &crate::utils::__INPUT_META_GetCurrentUnixTimestampInput,
    &crate::utils::__INPUT_META_GetCurrentIsoDatetimeInput,
    &crate::utils::__INPUT_META_GetCurrentFormattedDateTimeInput,
    &crate::utils::__INPUT_META_CountryNameToIsoCodeInput,
    #[cfg(feature = "native")]
    &crate::xlsx::__INPUT_META_FromXlsxInput,
    #[cfg(feature = "native")]
    &crate::xlsx::__INPUT_META_GetSheetsInput,
    &crate::xml::__INPUT_META_FromXmlInput,
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
    &crate::crypto::__OUTPUT_META_HashResult,
    &crate::datetime::__OUTPUT_META_TimeBetweenResult,
    &crate::datetime::__OUTPUT_META_UnixTimestampResult,
    &crate::file::__OUTPUT_META_WriteFileResponse,
    &crate::file::__OUTPUT_META_WorkspaceFileInfo,
    &crate::file::__OUTPUT_META_DeleteResponse,
    &crate::file::__OUTPUT_META_ExistsResponse,
    &crate::file::__OUTPUT_META_CopyResponse,
    &crate::file::__OUTPUT_META_MoveResponse,
    &crate::file::__OUTPUT_META_CreateDirResponse,
    &crate::file::__OUTPUT_META_FileMetadata,
    &crate::file::__OUTPUT_META_AppendFileResponse,
    &crate::http::__OUTPUT_META_HttpResponse,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__OUTPUT_META_AiTextCompletionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__OUTPUT_META_AiImageGenerationOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__OUTPUT_META_AiVisionToTextOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__OUTPUT_META_AiVisionToImageOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::ai_tools::__OUTPUT_META_AiEmbedTextOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_TextCompletionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_ImageGenerationOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_StructuredOutputOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_VisionToTextOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_VisionToImageOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_BedrockInvokeModelOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::bedrock::__OUTPUT_META_BedrockListModelsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceProduct,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceInventoryLevel,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceOrder,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceLocation,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceGetProductsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceGetLocationsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceDeleteProductOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::commerce::__OUTPUT_META_CommerceGetOrdersOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListContactsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetContactOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateContactOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_UpdateContactOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_DeleteContactOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_SearchContactsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListCompaniesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetCompanyOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateCompanyOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_UpdateCompanyOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_DeleteCompanyOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_SearchCompaniesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListDealsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetDealOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateDealOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_UpdateDealOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_DeleteDealOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_SearchDealsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListQuotesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetQuoteOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateQuoteOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_UpdateQuoteOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListLineItemsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateLineItemOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_DeleteLineItemOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListOwnersOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetOwnerOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListPipelinesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_GetPipelineOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_CreateAssociationOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::hubspot::__OUTPUT_META_ListAssociationsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::mailgun::__OUTPUT_META_SendEmailOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_CreateInstanceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_QueryInstancesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_CheckInstanceExistsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_CreateIfNotExistsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_UpdateInstanceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_DeleteInstanceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_BulkCreateInstancesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_BulkUpdateInstancesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_BulkDeleteInstancesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_QueryAggregateOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_LoadMemoryOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::object_model::__OUTPUT_META_SaveMemoryOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_TextCompletionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_ImageGenerationOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_StructuredOutputOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_VisionToTextOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_VisionToImageOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_OpenaiChatCompletionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_OpenaiCreateEmbeddingOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::openai::__OUTPUT_META_OpenaiModerateContentOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_CreateBucketOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_ListBucketsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_DeleteBucketOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_UploadFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_DownloadFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_ListFilesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_GetFileInfoOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_DeleteFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::s3_storage::__OUTPUT_META_CopyFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_ListDrivesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_ListChildrenOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_GetItemOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_DownloadFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_UploadFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_CreateFolderOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_DeleteItemOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_CopyItemOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_GetCopyStatusOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::sharepoint::__OUTPUT_META_SearchGlobalOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__OUTPUT_META_FulfillBySkuOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__OUTPUT_META_CommerceGetProductsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__OUTPUT_META_CommerceDeleteProductOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__OUTPUT_META_CommerceGetOrdersOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::shopify::__OUTPUT_META_CommerceGetLocationsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::slack::__OUTPUT_META_SendMessageOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::slack::__OUTPUT_META_UploadFileOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListCustomersOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetCustomerOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreateCustomerOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_UpdateCustomerOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListProductsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetProductOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreateProductOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListPricesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreatePriceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreatePaymentIntentOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetPaymentIntentOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListPaymentIntentsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreateInvoiceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetInvoiceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListInvoicesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_FinalizeInvoiceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_SendInvoiceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreateSubscriptionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetSubscriptionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListSubscriptionsOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CancelSubscriptionOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_CreateRefundOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetRefundOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetBalanceOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_ListChargesOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::stripe::__OUTPUT_META_GetChargeOutput,
    #[cfg(feature = "integrations")]
    &crate::integrations::types::__OUTPUT_META_FileData,
    #[cfg(feature = "integrations")]
    &crate::integrations::types::__OUTPUT_META_LlmUsage,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_FileInfo,
    #[cfg(feature = "native")]
    &crate::sftp::__OUTPUT_META_DeleteFileResponse,
    &crate::transform::__OUTPUT_META_ExtractOutput,
    &crate::transform::__OUTPUT_META_FilterOutput,
    &crate::transform::__OUTPUT_META_SortOutput,
    &crate::transform::__OUTPUT_META_GroupByOutput,
    &crate::transform::__OUTPUT_META_MapFieldsOutput,
    &crate::transform::__OUTPUT_META_AppendOutput,
    &crate::transform::__OUTPUT_META_FlatMapOutput,
    &crate::transform::__OUTPUT_META_ArrayLengthOutput,
    &crate::transform::__OUTPUT_META_ToJsonStringOutput,
    &crate::transform::__OUTPUT_META_EnsureArrayOutput,
    #[cfg(feature = "native")]
    &crate::xlsx::__OUTPUT_META_SheetInfo,
];

pub static CONNECTION_TYPES: &[&ConnectionTypeMeta] = &[
    &crate::extractors::http_api_key::__CONNECTION_META_HttpApiKeyParams,
    &crate::extractors::http_bearer::__CONNECTION_META_HttpBearerParams,
    &crate::extractors::sftp::__CONNECTION_META_SftpParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_ShopifyAccessTokenParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_ShopifyClientCredentialsParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_OpenAiApiKeyParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_AwsCredentialsParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_TelegramBotParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_SlackBotParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_TeamsBotParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_MicrosoftEntraClientCredentialsParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_MailgunParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_HubSpotPrivateAppParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_HubSpotAccessTokenParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_PostgresDatabaseParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_S3CompatibleParams,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::__CONNECTION_META_StripeApiKeyParams,
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

#[cfg(feature = "integrations")]
const AGENT_MODULE_AI_TOOLS: AgentModuleConfig = AgentModuleConfig {
    id: "ai_tools",
    name: "AI Tools",
    description: "AI tools - deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["openai_api_key", "aws_credentials"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_BEDROCK: AgentModuleConfig = AgentModuleConfig {
    id: "bedrock",
    name: "AWS Bedrock",
    description: "AWS Bedrock LLM integration for text completion, image generation, structured output, and vision capabilities using Claude and Titan models",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["aws_credentials"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_COMMERCE: AgentModuleConfig = AgentModuleConfig {
    id: "commerce",
    name: "Commerce",
    description: "Unified interface for product, order, and inventory management across multiple e-commerce platforms",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["shopify_access_token", "shopify_client_credentials"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_HUBSPOT: AgentModuleConfig = AgentModuleConfig {
    id: "hubspot",
    name: "HubSpot",
    description: "HubSpot CRM - manage contacts, companies, deals, quotes, and pipelines",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["hubspot_private_app", "hubspot_access_token"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_MAILGUN: AgentModuleConfig = AgentModuleConfig {
    id: "mailgun",
    name: "Mailgun",
    description: "Mailgun email service for sending transactional and marketing emails",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["mailgun"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_OPENAI: AgentModuleConfig = AgentModuleConfig {
    id: "openai",
    name: "OpenAI",
    description: "OpenAI LLM integration for text completion, image generation, structured output, and vision capabilities",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["openai_api_key"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_S3_STORAGE: AgentModuleConfig = AgentModuleConfig {
    id: "s3_storage",
    name: "S3 Storage",
    description: "S3-compatible object storage for file upload, download, listing, and bucket operations",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["s3_compatible"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_SHAREPOINT: AgentModuleConfig = AgentModuleConfig {
    id: "sharepoint",
    name: "Microsoft SharePoint",
    description: "Microsoft SharePoint - file management over Microsoft Graph",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["microsoft_entra_client_credentials"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_SHOPIFY: AgentModuleConfig = AgentModuleConfig {
    id: "shopify",
    name: "Shopify",
    description: "Shopify GraphQL Admin API integration for product, order, inventory, and customer operations",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["shopify_access_token", "shopify_client_credentials"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_SLACK: AgentModuleConfig = AgentModuleConfig {
    id: "slack",
    name: "Slack",
    description: "Slack messaging for sending messages, files, and reactions",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["slack_bot"],
    secure: true,
};

#[cfg(feature = "integrations")]
const AGENT_MODULE_STRIPE: AgentModuleConfig = AgentModuleConfig {
    id: "stripe",
    name: "Stripe",
    description: "Stripe payment platform - manage customers, payments, invoices, and subscriptions",
    has_side_effects: true,
    supports_connections: true,
    integration_ids: &["stripe_api_key"],
    secure: true,
};

pub static EXTRA_AGENT_MODULES: &[&AgentModuleConfig] = &[
    #[cfg(feature = "native")]
    &XLSX_AGENT_MODULE,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_AI_TOOLS,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_BEDROCK,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_COMMERCE,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_HUBSPOT,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_MAILGUN,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_OPENAI,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_S3_STORAGE,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_SHAREPOINT,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_SHOPIFY,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_SLACK,
    #[cfg(feature = "integrations")]
    &AGENT_MODULE_STRIPE,
];
