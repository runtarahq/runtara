// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
//! Static capability dispatch table for workflow binaries.
//!
//! This module provides a compile-time dispatch table that maps (module, capability_id)
//! pairs directly to executor functions. It replaces `inventory::iter()` at runtime
//! for workflow binaries, eliminating the `inventory` crate dependency from compiled
//! workflows.
//!
//! The server-side code (runtime HTTP server) continues to use inventory-based
//! dispatch via `registry::execute_capability()` for metadata APIs and dynamic
//! capability discovery.
//!
//! When adding a new capability with `#[capability]`, add a corresponding entry
//! to the match table below. A test (`test_dispatch_table_completeness`) verifies
//! that all inventory-registered capabilities have dispatch entries.

/// Execute a native-only capability via the agent service HTTP endpoint.
///
/// Used by WASM workflow binaries that cannot link native C libraries (sftp, xlsx,
/// compression). The stub POSTs the input to the runtime's internal agent service,
/// which executes the capability in the server process and returns the result.
#[cfg(not(feature = "native"))]
pub fn native_agent_stub(
    module: &str,
    capability_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Env vars are stable for the lifetime of a workflow process — cache on first read.
    use std::sync::OnceLock;
    static AGENT_SERVICE_URL: OnceLock<String> = OnceLock::new();
    static TENANT_ID: OnceLock<String> = OnceLock::new();

    let base_url = AGENT_SERVICE_URL.get_or_init(|| {
        std::env::var("RUNTARA_AGENT_SERVICE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:7002/api/internal/agents".to_string())
    });
    let url = format!("{}/{}/{}", base_url, module, capability_id);
    let tid = TENANT_ID.get_or_init(|| std::env::var("RUNTARA_TENANT_ID").unwrap_or_default());

    let client = runtara_http::HttpClient::new();
    let resp = client
        .request("POST", &url)
        .header("X-Org-Id", tid)
        .header("Content-Type", "application/json")
        .body_json(&input)
        .call()
        .map_err(|e| {
            format!(
                "Agent service request failed for {}:{}: {}",
                module, capability_id, e
            )
        })?;

    let body: serde_json::Value = resp.into_json().map_err(|e| {
        format!(
            "Failed to parse agent service response for {}:{}: {}",
            module, capability_id, e
        )
    })?;

    if body
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Ok(body
            .get("output")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    } else {
        Err(body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown agent service error")
            .to_string())
    }
}

/// Execute a capability by module and capability_id using static dispatch.
///
/// This is the workflow-binary equivalent of `registry::execute_capability()`.
/// Instead of iterating over inventory-registered executors at runtime, it uses
/// a compile-time match table for direct dispatch.
pub fn execute_capability(
    module: &str,
    capability_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let module_lower = module.to_lowercase();
    match (module_lower.as_str(), capability_id) {
        // =====================================================================
        // runtara-agents capabilities
        // =====================================================================
        // --- compression ---
        #[cfg(feature = "native")]
        ("compression", "create-archive") => {
            (runtara_agents::compression::__CAPABILITY_EXECUTOR_CREATE_ARCHIVE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("compression", "create-archive") => {
            native_agent_stub("compression", "create-archive", input)
        }
        #[cfg(feature = "native")]
        ("compression", "extract-archive") => {
            (runtara_agents::compression::__CAPABILITY_EXECUTOR_EXTRACT_ARCHIVE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("compression", "extract-archive") => {
            native_agent_stub("compression", "extract-archive", input)
        }
        #[cfg(feature = "native")]
        ("compression", "extract-file") => {
            (runtara_agents::compression::__CAPABILITY_EXECUTOR_EXTRACT_FILE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("compression", "extract-file") => native_agent_stub("compression", "extract-file", input),
        #[cfg(feature = "native")]
        ("compression", "list-archive") => {
            (runtara_agents::compression::__CAPABILITY_EXECUTOR_LIST_ARCHIVE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("compression", "list-archive") => native_agent_stub("compression", "list-archive", input),

        // --- crypto ---
        ("crypto", "hash") => (runtara_agents::crypto::__CAPABILITY_EXECUTOR_HASH.execute)(input),
        ("crypto", "hmac") => (runtara_agents::crypto::__CAPABILITY_EXECUTOR_HMAC.execute)(input),

        // --- csv ---
        ("csv", "from-csv") => (runtara_agents::csv::__CAPABILITY_EXECUTOR_FROM_CSV.execute)(input),
        ("csv", "get-header") => {
            (runtara_agents::csv::__CAPABILITY_EXECUTOR_GET_HEADER.execute)(input)
        }
        ("csv", "to-csv") => (runtara_agents::csv::__CAPABILITY_EXECUTOR_TO_CSV.execute)(input),

        // --- datetime ---
        ("datetime", "add-to-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_ADD_TO_DATE.execute)(input)
        }
        ("datetime", "date-to-unix") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_DATE_TO_UNIX.execute)(input)
        }
        ("datetime", "extract-date-part") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_EXTRACT_DATE_PART.execute)(input)
        }
        ("datetime", "format-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_FORMAT_DATE.execute)(input)
        }
        ("datetime", "get-current-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_GET_CURRENT_DATE.execute)(input)
        }
        ("datetime", "get-time-between") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_GET_TIME_BETWEEN.execute)(input)
        }
        ("datetime", "round-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_ROUND_DATE.execute)(input)
        }
        ("datetime", "subtract-from-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_SUBTRACT_FROM_DATE.execute)(input)
        }
        ("datetime", "unix-to-date") => {
            (runtara_agents::datetime::__CAPABILITY_EXECUTOR_UNIX_TO_DATE.execute)(input)
        }

        // --- file ---
        ("file", "file-append-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_APPEND_FILE.execute)(input)
        }
        ("file", "file-copy-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_COPY_FILE.execute)(input)
        }
        ("file", "file-create-directory") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_CREATE_DIRECTORY.execute)(input)
        }
        ("file", "file-delete-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_DELETE_FILE.execute)(input)
        }
        ("file", "file-file-exists") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_FILE_EXISTS.execute)(input)
        }
        ("file", "file-get-file-info") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_GET_FILE_INFO.execute)(input)
        }
        ("file", "file-list-files") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_LIST_FILES.execute)(input)
        }
        ("file", "file-move-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_MOVE_FILE.execute)(input)
        }
        ("file", "file-read-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_READ_FILE.execute)(input)
        }
        ("file", "file-write-file") => {
            (runtara_agents::file::__CAPABILITY_EXECUTOR_FILE_WRITE_FILE.execute)(input)
        }

        // --- http ---
        ("http", "http-request") => {
            (runtara_agents::http::__CAPABILITY_EXECUTOR_HTTP_REQUEST.execute)(input)
        }

        // --- sftp ---
        #[cfg(feature = "native")]
        ("sftp", "sftp-delete-file") => {
            (runtara_agents::sftp::__CAPABILITY_EXECUTOR_SFTP_DELETE_FILE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("sftp", "sftp-delete-file") => native_agent_stub("sftp", "sftp-delete-file", input),
        #[cfg(feature = "native")]
        ("sftp", "sftp-download-file") => {
            (runtara_agents::sftp::__CAPABILITY_EXECUTOR_SFTP_DOWNLOAD_FILE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("sftp", "sftp-download-file") => native_agent_stub("sftp", "sftp-download-file", input),
        #[cfg(feature = "native")]
        ("sftp", "sftp-list-files") => {
            (runtara_agents::sftp::__CAPABILITY_EXECUTOR_SFTP_LIST_FILES.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("sftp", "sftp-list-files") => native_agent_stub("sftp", "sftp-list-files", input),
        #[cfg(feature = "native")]
        ("sftp", "sftp-upload-file") => {
            (runtara_agents::sftp::__CAPABILITY_EXECUTOR_SFTP_UPLOAD_FILE.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("sftp", "sftp-upload-file") => native_agent_stub("sftp", "sftp-upload-file", input),

        // --- text ---
        ("text", "as-byte-array") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_AS_BYTE_ARRAY.execute)(input)
        }
        ("text", "case-conversion") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_CASE_CONVERSION.execute)(input)
        }
        ("text", "collapse-expand-lines") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_COLLAPSE_EXPAND_LINES.execute)(input)
        }
        ("text", "compare-text") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_COMPARE_TEXT.execute)(input)
        }
        ("text", "count-occurrences") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_COUNT_OCCURRENCES.execute)(input)
        }
        ("text", "extract-emails") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_EXTRACT_EMAILS.execute)(input)
        }
        ("text", "extract-first-line") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_EXTRACT_FIRST_LINE.execute)(input)
        }
        ("text", "extract-first-word") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_EXTRACT_FIRST_WORD.execute)(input)
        }
        ("text", "extract-numbers") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_EXTRACT_NUMBERS.execute)(input)
        }
        ("text", "extract-urls") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_EXTRACT_URLS.execute)(input)
        }
        ("text", "find-replace") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_FIND_REPLACE.execute)(input)
        }
        ("text", "from-base64") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_FROM_BASE64.execute)(input)
        }
        ("text", "hash-text") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_HASH_TEXT.execute)(input)
        }
        ("text", "pad-text") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_PAD_TEXT.execute)(input)
        }
        ("text", "regex-match") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_REGEX_MATCH.execute)(input)
        }
        ("text", "regex-replace") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_REGEX_REPLACE.execute)(input)
        }
        ("text", "regex-split") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_REGEX_SPLIT.execute)(input)
        }
        ("text", "regex-test") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_REGEX_TEST.execute)(input)
        }
        ("text", "remove-characters") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_REMOVE_CHARACTERS.execute)(input)
        }
        ("text", "render-template") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_RENDER_TEMPLATE.execute)(input)
        }
        ("text", "slugify") => (runtara_agents::text::__CAPABILITY_EXECUTOR_SLUGIFY.execute)(input),
        ("text", "split") => (runtara_agents::text::__CAPABILITY_EXECUTOR_SPLIT.execute)(input),
        ("text", "split-join") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_SPLIT_JOIN.execute)(input)
        }
        ("text", "substring-extraction") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_SUBSTRING_EXTRACTION.execute)(input)
        }
        ("text", "to-base64") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_TO_BASE64.execute)(input)
        }
        ("text", "trim-normalize") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_TRIM_NORMALIZE.execute)(input)
        }
        ("text", "truncate-text") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_TRUNCATE_TEXT.execute)(input)
        }
        ("text", "wrap-text") => {
            (runtara_agents::text::__CAPABILITY_EXECUTOR_WRAP_TEXT.execute)(input)
        }

        // --- transform ---
        ("transform", "append") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_APPEND.execute)(input)
        }
        ("transform", "array-length") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_ARRAY_LENGTH.execute)(input)
        }
        ("transform", "coalesce") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_COALESCE.execute)(input)
        }
        ("transform", "ensure-array") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_ENSURE_ARRAY.execute)(input)
        }
        ("transform", "extract") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_EXTRACT.execute)(input)
        }
        ("transform", "filter") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_FILTER.execute)(input)
        }
        ("transform", "filter-non-values") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_FILTER_NON_VALUES.execute)(input)
        }
        ("transform", "flat-map") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_FLAT_MAP.execute)(input)
        }
        ("transform", "from-json-string") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_FROM_JSON_STRING.execute)(input)
        }
        ("transform", "get-value-by-path") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_GET_VALUE_BY_PATH.execute)(input)
        }
        ("transform", "group-by") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_GROUP_BY.execute)(input)
        }
        ("transform", "map-fields") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_MAP_FIELDS.execute)(input)
        }
        ("transform", "select-first") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_SELECT_FIRST.execute)(input)
        }
        ("transform", "set-value-by-path") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_SET_VALUE_BY_PATH.execute)(input)
        }
        ("transform", "sort") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_SORT.execute)(input)
        }
        ("transform", "to-json-string") => {
            (runtara_agents::transform::__CAPABILITY_EXECUTOR_TO_JSON_STRING.execute)(input)
        }

        // --- utils ---
        ("utils", "calculate") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_CALCULATE.execute)(input)
        }
        ("utils", "country-name-to-iso-code") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_COUNTRY_NAME_TO_ISO_CODE.execute)(input)
        }
        ("utils", "delay-in-ms") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_DELAY_IN_MS.execute)(input)
        }
        ("utils", "do-nothing") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_DO_NOTHING.execute)(input)
        }
        ("utils", "format-date-from-iso") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_FORMAT_DATE_FROM_ISO.execute)(input)
        }
        ("utils", "get-current-formatted-datetime") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_FORMATTED_DATETIME.execute)(
                input,
            )
        }
        ("utils", "get-current-iso-datetime") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_ISO_DATETIME.execute)(input)
        }
        ("utils", "get-current-unix-timestamp") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_GET_CURRENT_UNIX_TIMESTAMP.execute)(input)
        }
        ("utils", "iso-to-unix-timestamp") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_ISO_TO_UNIX_TIMESTAMP.execute)(input)
        }
        ("utils", "random-array") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_RANDOM_ARRAY.execute)(input)
        }
        ("utils", "random-double") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_RANDOM_DOUBLE.execute)(input)
        }
        ("utils", "return-input") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_RETURN_INPUT.execute)(input)
        }
        ("utils", "return-input-string") => {
            (runtara_agents::utils::__CAPABILITY_EXECUTOR_RETURN_INPUT_STRING.execute)(input)
        }

        // --- xlsx ---
        #[cfg(feature = "native")]
        ("xlsx", "from-xlsx") => {
            (runtara_agents::xlsx::__CAPABILITY_EXECUTOR_FROM_XLSX.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("xlsx", "from-xlsx") => native_agent_stub("xlsx", "from-xlsx", input),
        #[cfg(feature = "native")]
        ("xlsx", "get-sheets") => {
            (runtara_agents::xlsx::__CAPABILITY_EXECUTOR_GET_SHEETS.execute)(input)
        }
        #[cfg(not(feature = "native"))]
        ("xlsx", "get-sheets") => native_agent_stub("xlsx", "get-sheets", input),

        // --- xml ---
        ("xml", "from-xml") => (runtara_agents::xml::__CAPABILITY_EXECUTOR_FROM_XML.execute)(input),

        // =====================================================================
        // smo-stdlib capabilities
        // =====================================================================
        // --- ai_tools ---
        ("ai_tools", "ai-embed-text") => {
            (runtara_agents::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_EMBED_TEXT.execute)(input)
        }
        ("ai_tools", "ai-image-generation") => {
            (runtara_agents::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_IMAGE_GENERATION.execute)(input)
        }
        ("ai_tools", "ai-text-completion") => {
            (runtara_agents::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_TEXT_COMPLETION.execute)(input)
        }
        ("ai_tools", "ai-vision-to-image") => {
            (runtara_agents::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_VISION_TO_IMAGE.execute)(input)
        }
        ("ai_tools", "ai-vision-to-text") => {
            (runtara_agents::integrations::ai_tools::__CAPABILITY_EXECUTOR_AI_VISION_TO_TEXT.execute)(input)
        }

        // --- bedrock ---
        ("bedrock", "bedrock-invoke-model") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_BEDROCK_INVOKE_MODEL.execute)(input)
        }
        ("bedrock", "bedrock-list-models") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_BEDROCK_LIST_MODELS.execute)(input)
        }
        ("bedrock", "image-generation") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_IMAGE_GENERATION.execute)(input)
        }
        ("bedrock", "structured-output") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_STRUCTURED_OUTPUT.execute)(input)
        }
        ("bedrock", "text-completion") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_TEXT_COMPLETION.execute)(input)
        }
        ("bedrock", "vision-to-image") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_VISION_TO_IMAGE.execute)(input)
        }
        ("bedrock", "vision-to-text") => {
            (runtara_agents::integrations::bedrock::__CAPABILITY_EXECUTOR_VISION_TO_TEXT.execute)(input)
        }

        // --- commerce ---
        ("commerce", "create-product") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_CREATE_PRODUCT.execute)(
                input,
            )
        }
        ("commerce", "delete-product") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_DELETE_PRODUCT.execute)(
                input,
            )
        }
        ("commerce", "get-inventory") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_INVENTORY.execute)(
                input,
            )
        }
        ("commerce", "get-locations") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_LOCATIONS.execute)(
                input,
            )
        }
        ("commerce", "get-order") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_ORDER.execute)(input)
        }
        ("commerce", "get-orders") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_ORDERS.execute)(input)
        }
        ("commerce", "get-product") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_PRODUCT.execute)(input)
        }
        ("commerce", "get-products") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_GET_PRODUCTS.execute)(input)
        }
        ("commerce", "update-inventory") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_UPDATE_INVENTORY.execute)(
                input,
            )
        }
        ("commerce", "update-product") => {
            (runtara_agents::integrations::commerce::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT.execute)(
                input,
            )
        }

        // --- hubspot ---
        ("hubspot", "create-association") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_ASSOCIATION.execute)(input)
        }
        ("hubspot", "create-company") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_COMPANY.execute)(input)
        }
        ("hubspot", "create-contact") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_CONTACT.execute)(input)
        }
        ("hubspot", "create-deal") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_DEAL.execute)(input)
        }
        ("hubspot", "create-line-item") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_LINE_ITEM.execute)(input)
        }
        ("hubspot", "create-quote") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_CREATE_QUOTE.execute)(input)
        }
        ("hubspot", "delete-company") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_COMPANY.execute)(input)
        }
        ("hubspot", "delete-contact") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_CONTACT.execute)(input)
        }
        ("hubspot", "delete-deal") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_DEAL.execute)(input)
        }
        ("hubspot", "delete-line-item") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_DELETE_LINE_ITEM.execute)(input)
        }
        ("hubspot", "get-company") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_COMPANY.execute)(input)
        }
        ("hubspot", "get-contact") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_CONTACT.execute)(input)
        }
        ("hubspot", "get-deal") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_DEAL.execute)(input)
        }
        ("hubspot", "get-owner") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_OWNER.execute)(input)
        }
        ("hubspot", "get-pipeline") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_PIPELINE.execute)(input)
        }
        ("hubspot", "get-quote") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_GET_QUOTE.execute)(input)
        }
        ("hubspot", "list-associations") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_ASSOCIATIONS.execute)(input)
        }
        ("hubspot", "list-companies") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_COMPANIES.execute)(input)
        }
        ("hubspot", "list-contacts") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_CONTACTS.execute)(input)
        }
        ("hubspot", "list-deals") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_DEALS.execute)(input)
        }
        ("hubspot", "list-line-items") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_LINE_ITEMS.execute)(input)
        }
        ("hubspot", "list-owners") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_OWNERS.execute)(input)
        }
        ("hubspot", "list-pipelines") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_PIPELINES.execute)(input)
        }
        ("hubspot", "list-quotes") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_LIST_QUOTES.execute)(input)
        }
        ("hubspot", "search-companies") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_COMPANIES.execute)(input)
        }
        ("hubspot", "search-contacts") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_CONTACTS.execute)(input)
        }
        ("hubspot", "search-deals") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_SEARCH_DEALS.execute)(input)
        }
        ("hubspot", "update-company") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_COMPANY.execute)(input)
        }
        ("hubspot", "update-contact") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_CONTACT.execute)(input)
        }
        ("hubspot", "update-deal") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_DEAL.execute)(input)
        }
        ("hubspot", "update-quote") => {
            (runtara_agents::integrations::hubspot::__CAPABILITY_EXECUTOR_UPDATE_QUOTE.execute)(input)
        }

        // --- mailgun ---
        ("mailgun", "send-email") => {
            (runtara_agents::integrations::mailgun::__CAPABILITY_EXECUTOR_SEND_EMAIL.execute)(input)
        }

        // --- object_model ---
        ("object_model", "check-instance-exists") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_CHECK_INSTANCE_EXISTS.execute)(
                input,
            )
        }
        ("object_model", "create-if-not-exists") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_CREATE_IF_NOT_EXISTS.execute)(
                input,
            )
        }
        ("object_model", "create-instance") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_CREATE_INSTANCE.execute)(input)
        }
        ("object_model", "load-memory") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_LOAD_MEMORY.execute)(input)
        }
        ("object_model", "query-instances") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_QUERY_INSTANCES.execute)(input)
        }
        ("object_model", "query-aggregate") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_QUERY_AGGREGATE.execute)(input)
        }
        ("object_model", "save-memory") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_SAVE_MEMORY.execute)(input)
        }
        ("object_model", "update-instance") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_UPDATE_INSTANCE.execute)(input)
        }
        ("object_model", "delete-instance") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_DELETE_INSTANCE.execute)(input)
        }
        ("object_model", "bulk-create-instances") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_CREATE_INSTANCES.execute)(
                input,
            )
        }
        ("object_model", "bulk-update-instances") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_UPDATE_INSTANCES.execute)(
                input,
            )
        }
        ("object_model", "bulk-delete-instances") => {
            (runtara_agents::integrations::object_model::__CAPABILITY_EXECUTOR_BULK_DELETE_INSTANCES.execute)(
                input,
            )
        }

        // --- openai ---
        ("openai", "image-generation") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_IMAGE_GENERATION.execute)(input)
        }
        ("openai", "structured-output") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_STRUCTURED_OUTPUT.execute)(input)
        }
        ("openai", "text-completion") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_TEXT_COMPLETION.execute)(input)
        }
        ("openai", "vision-to-image") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_VISION_TO_IMAGE.execute)(input)
        }
        ("openai", "vision-to-text") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_VISION_TO_TEXT.execute)(input)
        }
        ("openai", "openai-chat-completion") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_CHAT_COMPLETION.execute)(input)
        }
        ("openai", "openai-create-embedding") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_CREATE_EMBEDDING.execute)(
                input,
            )
        }
        ("openai", "openai-moderate-content") => {
            (runtara_agents::integrations::openai::__CAPABILITY_EXECUTOR_OPENAI_MODERATE_CONTENT.execute)(
                input,
            )
        }

        // --- s3_storage ---
        ("s3_storage", "storage-copy-file") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_COPY_FILE.execute)(input)
        }
        ("s3_storage", "storage-create-bucket") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_CREATE_BUCKET.execute)(
                input,
            )
        }
        ("s3_storage", "storage-delete-bucket") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DELETE_BUCKET.execute)(
                input,
            )
        }
        ("s3_storage", "storage-delete-file") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DELETE_FILE.execute)(
                input,
            )
        }
        ("s3_storage", "storage-download-file") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_DOWNLOAD_FILE.execute)(
                input,
            )
        }
        ("s3_storage", "storage-get-file-info") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_GET_FILE_INFO.execute)(
                input,
            )
        }
        ("s3_storage", "storage-list-buckets") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_LIST_BUCKETS.execute)(
                input,
            )
        }
        ("s3_storage", "storage-list-files") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_LIST_FILES.execute)(input)
        }
        ("s3_storage", "storage-upload-file") => {
            (runtara_agents::integrations::s3_storage::__CAPABILITY_EXECUTOR_STORAGE_UPLOAD_FILE.execute)(
                input,
            )
        }

        // --- shopify ---
        ("shopify", "add-products-to-collection") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_ADD_PRODUCTS_TO_COLLECTION.execute)(
                input,
            )
        }
        ("shopify", "bulk-create-products") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_CREATE_PRODUCTS.execute)(input)
        }
        ("shopify", "bulk-update-products") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_UPDATE_PRODUCTS.execute)(input)
        }
        ("shopify", "bulk-update-variant-prices") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_BULK_UPDATE_VARIANT_PRICES.execute)(
                input,
            )
        }
        ("shopify", "cancel-order") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_CANCEL_ORDER.execute)(input)
        }
        ("shopify", "create-collection") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_COLLECTION.execute)(input)
        }
        ("shopify", "create-draft-order") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_DRAFT_ORDER.execute)(input)
        }
        ("shopify", "create-order-note-or-tag") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_ORDER_NOTE_OR_TAG.execute)(
                input,
            )
        }
        ("shopify", "create-product-variant") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_CREATE_PRODUCT_VARIANT.execute)(
                input,
            )
        }
        ("shopify", "delete-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_DELETE_PRODUCT.execute)(input)
        }
        ("shopify", "delete-product-variant") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_DELETE_PRODUCT_VARIANT.execute)(
                input,
            )
        }
        ("shopify", "fulfill-by-sku") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_BY_SKU.execute)(input)
        }
        ("shopify", "fulfill-order") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_ORDER.execute)(input)
        }
        ("shopify", "fulfill-order-lines") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_FULFILL_ORDER_LINES.execute)(input)
        }
        ("shopify", "get-customer-by-email") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_CUSTOMER_BY_EMAIL.execute)(input)
        }
        ("shopify", "get-fulfillment-orders") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_FULFILLMENT_ORDERS.execute)(
                input,
            )
        }
        ("shopify", "get-inventory-item-id-by-variant-id") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_INVENTORY_ITEM_ID_BY_VARIANT_ID
                .execute)(input)
        }
        ("shopify", "get-location-by-name") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_LOCATION_BY_NAME.execute)(input)
        }
        ("shopify", "get-order") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_ORDER.execute)(input)
        }
        ("shopify", "get-order-list") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_ORDER_LIST.execute)(input)
        }
        ("shopify", "get-product-by-sku") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_BY_SKU.execute)(input)
        }
        ("shopify", "get-product-metafields") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_METAFIELDS.execute)(
                input,
            )
        }
        ("shopify", "get-product-options") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_OPTIONS.execute)(input)
        }
        ("shopify", "get-product-variant-by-sku") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_GET_PRODUCT_VARIANT_BY_SKU.execute)(
                input,
            )
        }
        ("shopify", "commerce-create-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_CREATE_PRODUCT.execute)(input)
        }
        ("shopify", "commerce-delete-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_DELETE_PRODUCT.execute)(input)
        }
        ("shopify", "commerce-get-inventory") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_INVENTORY.execute)(input)
        }
        ("shopify", "commerce-get-locations") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_LOCATIONS.execute)(input)
        }
        ("shopify", "commerce-get-order") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_ORDER.execute)(input)
        }
        ("shopify", "commerce-get-orders") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_ORDERS.execute)(input)
        }
        ("shopify", "commerce-get-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_PRODUCT.execute)(input)
        }
        ("shopify", "commerce-get-products") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_GET_PRODUCTS.execute)(input)
        }
        ("shopify", "commerce-update-inventory") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_UPDATE_INVENTORY.execute)(input)
        }
        ("shopify", "commerce-update-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_COMMERCE_UPDATE_PRODUCT.execute)(input)
        }
        ("shopify", "list-products") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_LIST_PRODUCTS.execute)(input)
        }
        ("shopify", "query-products") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_QUERY_PRODUCTS.execute)(input)
        }
        ("shopify", "remove-products-from-collection") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_REMOVE_PRODUCTS_FROM_COLLECTION
                .execute)(input)
        }
        ("shopify", "rename-product-option") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_RENAME_PRODUCT_OPTION.execute)(input)
        }
        ("shopify", "replace-product-images") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_REPLACE_PRODUCT_IMAGES.execute)(
                input,
            )
        }
        ("shopify", "set-inventory") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_INVENTORY.execute)(input)
        }
        ("shopify", "set-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT.execute)(input)
        }
        ("shopify", "set-product-metafields") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_METAFIELDS.execute)(
                input,
            )
        }
        ("shopify", "set-product-tags") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_TAGS.execute)(input)
        }
        ("shopify", "set-product-variant-cost") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_VARIANT_COST.execute)(
                input,
            )
        }
        ("shopify", "set-product-variant-weight") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_PRODUCT_VARIANT_WEIGHT.execute)(
                input,
            )
        }
        ("shopify", "set-variant-metafields") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SET_VARIANT_METAFIELDS.execute)(
                input,
            )
        }
        ("shopify", "sync-inventory-levels") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_SYNC_INVENTORY_LEVELS.execute)(input)
        }
        ("shopify", "update-product") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT.execute)(input)
        }
        ("shopify", "update-product-variant") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT_VARIANT.execute)(
                input,
            )
        }
        ("shopify", "update-product-variant-price") => {
            (runtara_agents::integrations::shopify::__CAPABILITY_EXECUTOR_UPDATE_PRODUCT_VARIANT_PRICE.execute)(
                input,
            )
        }

        // --- slack ---
        ("slack", "send-message") => {
            (runtara_agents::integrations::slack::__CAPABILITY_EXECUTOR_SEND_MESSAGE.execute)(input)
        }
        ("slack", "upload-file") => {
            (runtara_agents::integrations::slack::__CAPABILITY_EXECUTOR_UPLOAD_FILE.execute)(input)
        }

        // --- stripe ---
        ("stripe", "cancel-subscription") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CANCEL_SUBSCRIPTION.execute)(input)
        }
        ("stripe", "create-customer") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_CUSTOMER.execute)(input)
        }
        ("stripe", "create-invoice") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_INVOICE.execute)(input)
        }
        ("stripe", "create-payment-intent") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PAYMENT_INTENT.execute)(input)
        }
        ("stripe", "create-price") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PRICE.execute)(input)
        }
        ("stripe", "create-product") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_PRODUCT.execute)(input)
        }
        ("stripe", "create-refund") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_REFUND.execute)(input)
        }
        ("stripe", "create-subscription") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_CREATE_SUBSCRIPTION.execute)(input)
        }
        ("stripe", "finalize-invoice") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_FINALIZE_INVOICE.execute)(input)
        }
        ("stripe", "get-balance") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_BALANCE.execute)(input)
        }
        ("stripe", "get-charge") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_CHARGE.execute)(input)
        }
        ("stripe", "get-customer") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_CUSTOMER.execute)(input)
        }
        ("stripe", "get-invoice") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_INVOICE.execute)(input)
        }
        ("stripe", "get-payment-intent") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_PAYMENT_INTENT.execute)(input)
        }
        ("stripe", "get-product") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_PRODUCT.execute)(input)
        }
        ("stripe", "get-refund") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_REFUND.execute)(input)
        }
        ("stripe", "get-subscription") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_GET_SUBSCRIPTION.execute)(input)
        }
        ("stripe", "list-charges") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_CHARGES.execute)(input)
        }
        ("stripe", "list-customers") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_CUSTOMERS.execute)(input)
        }
        ("stripe", "list-invoices") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_INVOICES.execute)(input)
        }
        ("stripe", "list-payment-intents") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PAYMENT_INTENTS.execute)(input)
        }
        ("stripe", "list-prices") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PRICES.execute)(input)
        }
        ("stripe", "list-products") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_PRODUCTS.execute)(input)
        }
        ("stripe", "list-subscriptions") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_LIST_SUBSCRIPTIONS.execute)(input)
        }
        ("stripe", "send-invoice") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_SEND_INVOICE.execute)(input)
        }
        ("stripe", "update-customer") => {
            (runtara_agents::integrations::stripe::__CAPABILITY_EXECUTOR_UPDATE_CUSTOMER.execute)(input)
        }

        _ => Err(format!("Unknown capability: {}:{}", module, capability_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that every capability registered via inventory has a corresponding
    /// entry in the static dispatch table. This catches missing entries when new
    /// capabilities are added.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn test_dispatch_table_completeness() {
        let dummy_input = serde_json::json!({});

        for executor in inventory::iter::<&'static runtara_dsl::agent_meta::CapabilityExecutor> {
            let result =
                execute_capability(executor.module, executor.capability_id, dummy_input.clone());
            // We don't care about the result (it will likely fail due to bad input),
            // but it must NOT return "Unknown capability" error
            if let Err(ref e) = result {
                assert!(
                    !e.starts_with("Unknown capability:"),
                    "Dispatch table missing entry for {}:{} — add it to dispatch.rs",
                    executor.module,
                    executor.capability_id
                );
            }
        }
    }
}
