// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Proc macros for runtara-sdk.
//!
//! Provides the `#[durable]` attribute macro for transparent durability with retry support.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{
    FnArg, Ident, ItemFn, LitInt, Pat, PatType, ReturnType, Token, Type, parse_macro_input,
    spanned::Spanned,
};

/// Parsed configuration from `#[durable(...)]` attributes.
#[derive(Default)]
struct DurableAttr {
    /// Maximum number of retry attempts (default: 3)
    max_retries: Option<u32>,
    /// Retry strategy (default: ExponentialBackoff)
    strategy: Option<String>,
    /// Base delay between retries in milliseconds (default: 1000)
    delay: Option<u64>,
}

impl Parse for DurableAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attr = DurableAttr::default();

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match ident.to_string().as_str() {
                "max_retries" => {
                    let lit: LitInt = input.parse()?;
                    attr.max_retries = Some(lit.base10_parse()?);
                }
                "strategy" => {
                    let strategy_ident: Ident = input.parse()?;
                    let strategy_str = strategy_ident.to_string();
                    if strategy_str != "ExponentialBackoff" {
                        return Err(syn::Error::new(
                            strategy_ident.span(),
                            "Only ExponentialBackoff strategy is currently supported",
                        ));
                    }
                    attr.strategy = Some(strategy_str);
                }
                "delay" => {
                    let lit: LitInt = input.parse()?;
                    attr.delay = Some(lit.base10_parse()?);
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "Unknown attribute '{}'. Valid attributes: max_retries, strategy, delay",
                            ident
                        ),
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(attr)
    }
}

/// Makes an async function durable by wrapping it with checkpoint-based caching and retry support.
///
/// The macro automatically:
/// - Checks for existing checkpoint before execution
/// - Returns cached result if checkpoint exists
/// - Retries the function on failure (if max_retries > 0)
/// - Records retry attempts to runtara-core for audit trail
/// - Executes function and saves result as checkpoint on success
///
/// # Requirements
///
/// - Function must be async
/// - **First parameter is the idempotency key** (any type that implements `Display`)
/// - Function must return `Result<T, E>` where `T: Serialize + DeserializeOwned`
/// - SDK must be registered via `RuntaraSdk::init()` before calling
///
/// # Example - Basic (no retries)
///
/// ```ignore
/// use runtara_sdk::durable;
///
/// #[durable]
/// pub async fn fetch_order(key: &str, order_id: &str) -> Result<Order, OrderError> {
///     // The key determines caching - same key = same cached result
///     db.fetch_order(order_id).await
/// }
/// ```
///
/// # Example - With retries
///
/// ```ignore
/// use runtara_sdk::durable;
///
/// #[durable(max_retries = 3, strategy = ExponentialBackoff, delay = 1000)]
/// pub async fn submit_order(key: &str, order: &Order) -> Result<OrderResult, OrderError> {
///     // Retries up to 3 times with exponential backoff:
///     // - First retry: 1000ms delay
///     // - Second retry: 2000ms delay
///     // - Third retry: 4000ms delay
///     external_service.submit(order).await
/// }
/// ```
#[proc_macro_attribute]
pub fn durable(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = parse_macro_input!(attr as DurableAttr);
    let input = parse_macro_input!(item as ItemFn);

    match generate_durable_wrapper(input, config) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate_durable_wrapper(input: ItemFn, config: DurableAttr) -> syn::Result<TokenStream2> {
    let fn_name = &input.sig.ident;
    let fn_name_str = fn_name.to_string();
    let vis = &input.vis;
    let attrs = &input.attrs;
    let sig = &input.sig;
    let block = &input.block;

    // Must be async
    if sig.asyncness.is_none() {
        return Err(syn::Error::new(
            sig.fn_token.span,
            "#[durable] only works with async functions",
        ));
    }

    // Validate return type is Result<T, E>
    let ok_type = extract_result_ok_type(&sig.output)?;

    // Extract the first argument as the idempotency key
    let idempotency_key_ident = extract_first_arg_ident(&sig.inputs)?;

    // Get retry configuration with defaults
    let max_retries = config.max_retries.unwrap_or(3);
    let base_delay_ms = config.delay.unwrap_or(1000);

    // Generate appropriate code based on whether retries are enabled
    if max_retries == 0 {
        // No retries - use simpler code path (original behavior)
        generate_no_retry_wrapper(
            fn_name_str,
            vis,
            attrs,
            sig,
            block,
            ok_type,
            idempotency_key_ident,
        )
    } else {
        // With retries - generate retry loop
        generate_retry_wrapper(
            fn_name_str,
            vis,
            attrs,
            sig,
            block,
            ok_type,
            idempotency_key_ident,
            max_retries,
            base_delay_ms,
        )
    }
}

/// Generate wrapper without retry logic (original behavior, for max_retries = 0)
fn generate_no_retry_wrapper(
    fn_name_str: String,
    vis: &syn::Visibility,
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
    block: &syn::Block,
    ok_type: Type,
    idempotency_key_ident: Ident,
) -> syn::Result<TokenStream2> {
    Ok(quote! {
        #(#attrs)*
        #vis #sig {
            let __cache_key = format!("durable::{}::{}", #fn_name_str, #idempotency_key_ident);

            // Step 1: Check if we have a cached result (read-only lookup)
            {
                let __sdk = ::runtara_sdk::sdk();
                let __sdk_guard = __sdk.lock().await;

                match __sdk_guard.get_checkpoint(&__cache_key).await {
                    Ok(Some(cached_bytes)) => {
                        // Found cached result - deserialize and return
                        drop(__sdk_guard);
                        match ::serde_json::from_slice::<#ok_type>(&cached_bytes) {
                            Ok(cached_value) => {
                                ::tracing::debug!(
                                    function = #fn_name_str,
                                    cache_key = %__cache_key,
                                    "Returning cached result from checkpoint"
                                );
                                return Ok(cached_value);
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    error = %e,
                                    "Failed to deserialize cached result, re-executing"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        // No cached result - will execute function
                    }
                    Err(e) => {
                        // Checkpoint lookup error - log and continue with execution
                        ::tracing::warn!(
                            function = #fn_name_str,
                            error = %e,
                            "Checkpoint lookup failed, executing function"
                        );
                    }
                }
            }

            // Step 2: Execute original function body
            let __result: std::result::Result<_, _> = async #block.await;

            // Step 3: Cache successful result
            if let Ok(ref value) = __result {
                match ::serde_json::to_vec(value) {
                    Ok(result_bytes) => {
                        let __sdk = ::runtara_sdk::sdk();
                        let __sdk_guard = __sdk.lock().await;

                        // Use checkpoint to save - it won't overwrite if already exists
                        match __sdk_guard.checkpoint(&__cache_key, &result_bytes).await {
                            Ok(checkpoint_result) => {
                                ::tracing::debug!(
                                    function = #fn_name_str,
                                    cache_key = %__cache_key,
                                    "Result cached via checkpoint"
                                );

                                // Check for pending pause/cancel signals
                                if checkpoint_result.should_cancel() {
                                    ::tracing::info!(
                                        function = #fn_name_str,
                                        "Cancel signal pending - instance should exit"
                                    );
                                } else if checkpoint_result.should_pause() {
                                    ::tracing::info!(
                                        function = #fn_name_str,
                                        "Pause signal pending - instance should exit after returning"
                                    );
                                }
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    error = %e,
                                    "Failed to cache result via checkpoint"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        ::tracing::warn!(
                            function = #fn_name_str,
                            error = %e,
                            "Failed to serialize result for caching"
                        );
                    }
                }
            }

            __result
        }
    })
}

/// Generate wrapper with retry logic
#[allow(clippy::too_many_arguments)]
fn generate_retry_wrapper(
    fn_name_str: String,
    vis: &syn::Visibility,
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
    block: &syn::Block,
    ok_type: Type,
    idempotency_key_ident: Ident,
    max_retries: u32,
    base_delay_ms: u64,
) -> syn::Result<TokenStream2> {
    let total_attempts = max_retries + 1;

    // Extract parameters that need cloning for retry loop
    let clonable_params = extract_clonable_params(&sig.inputs);

    // Generate clone statements for each clonable parameter
    let clone_statements: Vec<TokenStream2> = clonable_params
        .iter()
        .map(|(ident, _ty)| {
            quote! {
                let #ident = #ident.clone();
            }
        })
        .collect();

    Ok(quote! {
        #(#attrs)*
        #vis #sig {
            let __cache_key = format!("durable::{}::{}", #fn_name_str, #idempotency_key_ident);
            let __max_retries: u32 = #max_retries;
            let __base_delay_ms: u64 = #base_delay_ms;

            // Step 1: Check if we have a cached result (read-only lookup)
            {
                let __sdk = ::runtara_sdk::sdk();
                let __sdk_guard = __sdk.lock().await;

                match __sdk_guard.get_checkpoint(&__cache_key).await {
                    Ok(Some(cached_bytes)) => {
                        // Found cached result - deserialize and return
                        drop(__sdk_guard);
                        match ::serde_json::from_slice::<#ok_type>(&cached_bytes) {
                            Ok(cached_value) => {
                                ::tracing::debug!(
                                    function = #fn_name_str,
                                    cache_key = %__cache_key,
                                    "Returning cached result from checkpoint"
                                );
                                return Ok(cached_value);
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    error = %e,
                                    "Failed to deserialize cached result, re-executing"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        // No cached result - will execute with retries
                    }
                    Err(e) => {
                        // Checkpoint lookup error - log and continue with execution
                        ::tracing::warn!(
                            function = #fn_name_str,
                            error = %e,
                            "Checkpoint lookup failed, executing function"
                        );
                    }
                }
            }

            // Step 2: Retry loop
            let mut __last_error: Option<String> = None;
            let __total_attempts: u32 = #total_attempts;

            for __attempt in 1..=__total_attempts {
                // Clone non-reference parameters for this iteration
                #(#clone_statements)*

                // Record retry attempt (for attempts > 1)
                if __attempt > 1 {
                    // Apply backoff delay before retry
                    let __delay_multiplier = 2u64.pow(__attempt - 2);
                    let __delay = ::std::time::Duration::from_millis(
                        __base_delay_ms.saturating_mul(__delay_multiplier)
                    );

                    ::tracing::info!(
                        function = #fn_name_str,
                        attempt = __attempt,
                        max_retries = __max_retries,
                        delay_ms = __delay.as_millis() as u64,
                        last_error = ?__last_error,
                        "Retrying after backoff"
                    );

                    ::tokio::time::sleep(__delay).await;

                    // Record retry attempt to runtara-core
                    {
                        let __sdk = ::runtara_sdk::sdk();
                        let __sdk_guard = __sdk.lock().await;
                        if let Err(e) = __sdk_guard.record_retry_attempt(
                            &__cache_key,
                            __attempt,
                            __last_error.as_deref(),
                        ).await {
                            ::tracing::warn!(
                                function = #fn_name_str,
                                error = %e,
                                "Failed to record retry attempt"
                            );
                        }
                    }
                }

                // Execute the function body
                let __result: std::result::Result<_, _> = async #block.await;

                match __result {
                    Ok(ref value) => {
                        // Success - save checkpoint and return
                        match ::serde_json::to_vec(value) {
                            Ok(result_bytes) => {
                                let __sdk = ::runtara_sdk::sdk();
                                let __sdk_guard = __sdk.lock().await;

                                match __sdk_guard.checkpoint(&__cache_key, &result_bytes).await {
                                    Ok(checkpoint_result) => {
                                        ::tracing::debug!(
                                            function = #fn_name_str,
                                            cache_key = %__cache_key,
                                            attempts = __attempt,
                                            "Result cached via checkpoint"
                                        );

                                        // Check for pending pause/cancel signals
                                        if checkpoint_result.should_cancel() {
                                            ::tracing::info!(
                                                function = #fn_name_str,
                                                "Cancel signal pending - instance should exit"
                                            );
                                        } else if checkpoint_result.should_pause() {
                                            ::tracing::info!(
                                                function = #fn_name_str,
                                                "Pause signal pending - instance should exit after returning"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        ::tracing::warn!(
                                            function = #fn_name_str,
                                            error = %e,
                                            "Failed to cache result via checkpoint"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    error = %e,
                                    "Failed to serialize result for caching"
                                );
                            }
                        }
                        return __result;
                    }
                    Err(ref e) => {
                        __last_error = Some(format!("{}", e));

                        if __attempt < __total_attempts {
                            ::tracing::warn!(
                                function = #fn_name_str,
                                attempt = __attempt,
                                max_retries = __max_retries,
                                error = %e,
                                "Attempt failed, will retry"
                            );
                            continue;
                        } else {
                            ::tracing::error!(
                                function = #fn_name_str,
                                attempts = __attempt,
                                error = %e,
                                "All retry attempts exhausted"
                            );
                            return __result;
                        }
                    }
                }
            }

            // This should never be reached, but needed for type checker
            unreachable!("Retry loop should always return")
        }
    })
}

fn extract_result_ok_type(return_type: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = return_type else {
        return Err(syn::Error::new(
            return_type.span(),
            "#[durable] requires function to return Result<T, E>",
        ));
    };

    let Type::Path(type_path) = ty.as_ref() else {
        return Err(syn::Error::new(
            ty.span(),
            "#[durable] requires function to return Result<T, E>",
        ));
    };

    let segment = type_path.path.segments.last().ok_or_else(|| {
        syn::Error::new(
            ty.span(),
            "#[durable] requires function to return Result<T, E>",
        )
    })?;

    if segment.ident != "Result" {
        return Err(syn::Error::new(
            segment.ident.span(),
            "#[durable] requires function to return Result<T, E>",
        ));
    }

    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new(
            segment.span(),
            "#[durable] requires Result<T, E> with explicit type parameters",
        ));
    };

    match args.args.first() {
        Some(syn::GenericArgument::Type(t)) => Ok(t.clone()),
        _ => Err(syn::Error::new(
            args.span(),
            "#[durable] requires Result<T, E> with explicit type parameters",
        )),
    }
}

fn extract_first_arg_ident(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> syn::Result<syn::Ident> {
    // Skip `self` receiver if present, get first real argument
    for arg in inputs.iter() {
        match arg {
            FnArg::Receiver(_) => continue,
            FnArg::Typed(pat_type) => {
                let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
                    return Err(syn::Error::new(
                        pat_type.pat.span(),
                        "#[durable] requires the first argument to be a simple identifier",
                    ));
                };
                return Ok(pat_ident.ident.clone());
            }
        }
    }

    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "#[durable] requires at least one argument: the idempotency key (String)",
    ))
}

/// Check if a type is a reference type (starts with &)
fn is_reference_type(ty: &Type) -> bool {
    matches!(ty, Type::Reference(_))
}

/// Extract parameters that need to be cloned for retry loops.
/// Returns (ident, type) pairs for non-reference, non-first-arg parameters.
fn extract_clonable_params(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Vec<(Ident, Type)> {
    let mut params = Vec::new();
    let mut is_first = true;

    for arg in inputs.iter() {
        match arg {
            FnArg::Receiver(_) => continue,
            FnArg::Typed(PatType { pat, ty, .. }) => {
                // Skip the first argument (idempotency key)
                if is_first {
                    is_first = false;
                    continue;
                }

                // Skip reference types - they don't need cloning
                if is_reference_type(ty) {
                    continue;
                }

                // Extract the identifier
                if let Pat::Ident(pat_ident) = pat.as_ref() {
                    params.push((pat_ident.ident.clone(), (**ty).clone()));
                }
            }
        }
    }

    params
}
