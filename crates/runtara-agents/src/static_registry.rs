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
        meta: &crate::http::__CAPABILITY_META_HTTP_REQUEST,
        input_type: &crate::http::__INPUT_META_HttpRequestInput,
        #[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
        executor: &crate::http::__CAPABILITY_EXECUTOR_HTTP_REQUEST,
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
    &crate::http::__INPUT_META_HttpRequestInput,
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
    &crate::http::__OUTPUT_META_HttpResponse,
    &crate::types::__OUTPUT_META_LlmUsage,
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
    &crate::extractors::connection_types::__CONNECTION_META_ShopifyAccessTokenParams,
    &crate::extractors::connection_types::__CONNECTION_META_ShopifyClientCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_OpenAiApiKeyParams,
    &crate::extractors::connection_types::__CONNECTION_META_AwsCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_TelegramBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_SlackBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_TeamsBotParams,
    &crate::extractors::connection_types::__CONNECTION_META_MicrosoftEntraClientCredentialsParams,
    &crate::extractors::connection_types::__CONNECTION_META_MailgunParams,
    &crate::extractors::connection_types::__CONNECTION_META_HubSpotPrivateAppParams,
    &crate::extractors::connection_types::__CONNECTION_META_HubSpotAccessTokenParams,
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
