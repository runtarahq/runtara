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
#[derive(Debug, Default)]
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

                                // Release SDK mutex BEFORE calling acknowledge_cancellation()
                                // to prevent deadlock (it needs to acquire the same mutex)
                                drop(__sdk_guard);

                                // Check for pending pause/cancel signals
                                if checkpoint_result.should_cancel() {
                                    ::tracing::info!(
                                        function = #fn_name_str,
                                        "Cancel signal detected - exiting"
                                    );
                                    // Acknowledge cancellation to core (sets status to "cancelled")
                                    // and trigger local cancellation token
                                    ::runtara_sdk::acknowledge_cancellation().await;
                                    // Return error immediately to stop execution
                                    return Err("Instance cancelled".to_string().into());
                                } else if checkpoint_result.should_pause() {
                                    ::tracing::info!(
                                        function = #fn_name_str,
                                        "Pause signal detected - exiting"
                                    );
                                    // Return error to trigger exit; caller should call sdk.suspended()
                                    return Err("Instance paused".to_string().into());
                                }
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    cache_key = %__cache_key,
                                    error = %e,
                                    "Failed to cache result via checkpoint"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        ::tracing::warn!(
                            function = #fn_name_str,
                            cache_key = %__cache_key,
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
                        cache_key = %__cache_key,
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
                                cache_key = %__cache_key,
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
                                            attempt = __attempt,
                                            "Result cached via checkpoint"
                                        );

                                        // Release SDK mutex BEFORE calling acknowledge_cancellation()
                                        // to prevent deadlock (it needs to acquire the same mutex)
                                        drop(__sdk_guard);

                                        // Check for pending pause/cancel signals
                                        if checkpoint_result.should_cancel() {
                                            ::tracing::info!(
                                                function = #fn_name_str,
                                                "Cancel signal detected - exiting"
                                            );
                                            // Acknowledge cancellation to core (sets status to "cancelled")
                                            // and trigger local cancellation token
                                            ::runtara_sdk::acknowledge_cancellation().await;
                                            // Return error immediately to stop execution
                                            return Err("Instance cancelled".to_string().into());
                                        } else if checkpoint_result.should_pause() {
                                            ::tracing::info!(
                                                function = #fn_name_str,
                                                "Pause signal detected - exiting"
                                            );
                                            // Return error to trigger exit; caller should call sdk.suspended()
                                            return Err("Instance paused".to_string().into());
                                        }
                                    }
                                    Err(e) => {
                                        ::tracing::warn!(
                                            function = #fn_name_str,
                                            cache_key = %__cache_key,
                                            attempt = __attempt,
                                            error = %e,
                                            "Failed to cache result via checkpoint"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                ::tracing::warn!(
                                    function = #fn_name_str,
                                    cache_key = %__cache_key,
                                    attempt = __attempt,
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
                                cache_key = %__cache_key,
                                attempt = __attempt,
                                max_retries = __max_retries,
                                error = %e,
                                "Attempt failed, will retry"
                            );
                            continue;
                        } else {
                            ::tracing::error!(
                                function = #fn_name_str,
                                cache_key = %__cache_key,
                                attempt = __attempt,
                                max_retries = __max_retries,
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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_durable_attr_parsing_empty() {
        let attr: DurableAttr = syn::parse2(quote! {}).unwrap();
        assert!(attr.max_retries.is_none());
        assert!(attr.strategy.is_none());
        assert!(attr.delay.is_none());
    }

    #[test]
    fn test_durable_attr_parsing_max_retries() {
        let attr: DurableAttr = syn::parse2(quote! { max_retries = 5 }).unwrap();
        assert_eq!(attr.max_retries, Some(5));
        assert!(attr.strategy.is_none());
        assert!(attr.delay.is_none());
    }

    #[test]
    fn test_durable_attr_parsing_delay() {
        let attr: DurableAttr = syn::parse2(quote! { delay = 2000 }).unwrap();
        assert!(attr.max_retries.is_none());
        assert!(attr.strategy.is_none());
        assert_eq!(attr.delay, Some(2000));
    }

    #[test]
    fn test_durable_attr_parsing_strategy() {
        let attr: DurableAttr = syn::parse2(quote! { strategy = ExponentialBackoff }).unwrap();
        assert!(attr.max_retries.is_none());
        assert_eq!(attr.strategy, Some("ExponentialBackoff".to_string()));
        assert!(attr.delay.is_none());
    }

    #[test]
    fn test_durable_attr_parsing_all_options() {
        let attr: DurableAttr =
            syn::parse2(quote! { max_retries = 3, strategy = ExponentialBackoff, delay = 1000 })
                .unwrap();
        assert_eq!(attr.max_retries, Some(3));
        assert_eq!(attr.strategy, Some("ExponentialBackoff".to_string()));
        assert_eq!(attr.delay, Some(1000));
    }

    #[test]
    fn test_durable_attr_parsing_unknown_attribute_fails() {
        let result: Result<DurableAttr, _> = syn::parse2(quote! { unknown = 5 });
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown attribute"));
    }

    #[test]
    fn test_durable_attr_parsing_invalid_strategy_fails() {
        let result: Result<DurableAttr, _> = syn::parse2(quote! { strategy = LinearBackoff });
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Only ExponentialBackoff"));
    }

    #[test]
    fn test_extract_result_ok_type_valid() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) -> Result<String, Error> {
                Ok("hello".to_string())
            }
        };
        let result = extract_result_ok_type(&fn_item.sig.output);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_result_ok_type_no_return() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) {
            }
        };
        let result = extract_result_ok_type(&fn_item.sig.output);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_result_ok_type_not_result() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) -> Option<String> {
                Some("hello".to_string())
            }
        };
        let result = extract_result_ok_type(&fn_item.sig.output);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_first_arg_ident_valid() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str, value: i32) -> Result<(), ()> {
                Ok(())
            }
        };
        let result = extract_first_arg_ident(&fn_item.sig.inputs);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_string(), "key");
    }

    #[test]
    fn test_extract_first_arg_ident_no_args() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo() -> Result<(), ()> {
                Ok(())
            }
        };
        let result = extract_first_arg_ident(&fn_item.sig.inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_first_arg_ident_with_self() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(&self, key: &str) -> Result<(), ()> {
                Ok(())
            }
        };
        let result = extract_first_arg_ident(&fn_item.sig.inputs);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_string(), "key");
    }

    #[test]
    fn test_is_reference_type_reference() {
        let ty: Type = parse_quote! { &str };
        assert!(is_reference_type(&ty));
    }

    #[test]
    fn test_is_reference_type_not_reference() {
        let ty: Type = parse_quote! { String };
        assert!(!is_reference_type(&ty));
    }

    #[test]
    fn test_extract_clonable_params_reference_skipped() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str, value: &[u8]) -> Result<(), ()> {
                Ok(())
            }
        };
        let params = extract_clonable_params(&fn_item.sig.inputs);
        // Both args are references, so nothing should be cloned
        // (first arg is also skipped as idempotency key)
        assert!(params.is_empty());
    }

    #[test]
    fn test_extract_clonable_params_owned_included() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str, value: String, count: i32) -> Result<(), ()> {
                Ok(())
            }
        };
        let params = extract_clonable_params(&fn_item.sig.inputs);
        // First arg (key) is skipped, value and count should be included
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0.to_string(), "value");
        assert_eq!(params[1].0.to_string(), "count");
    }

    #[test]
    fn test_generate_durable_wrapper_not_async_fails() {
        let fn_item: ItemFn = parse_quote! {
            fn foo(key: &str) -> Result<(), ()> {
                Ok(())
            }
        };
        let config = DurableAttr::default();
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("async"));
    }

    #[test]
    fn test_generate_durable_wrapper_valid() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) -> Result<String, String> {
                Ok("hello".to_string())
            }
        };
        let config = DurableAttr::default();
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_durable_wrapper_zero_retries() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) -> Result<String, String> {
                Ok("hello".to_string())
            }
        };
        let config = DurableAttr {
            max_retries: Some(0),
            strategy: None,
            delay: None,
        };
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
        // Should generate the no-retry path
        let tokens = result.unwrap().to_string();
        // Check that retry-specific code is NOT present
        assert!(!tokens.contains("__max_retries"));
    }

    #[test]
    fn test_generate_durable_wrapper_with_retries() {
        let fn_item: ItemFn = parse_quote! {
            async fn foo(key: &str) -> Result<String, String> {
                Ok("hello".to_string())
            }
        };
        let config = DurableAttr {
            max_retries: Some(3),
            strategy: None,
            delay: Some(1000),
        };
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
        // Should generate the retry path
        let tokens = result.unwrap().to_string();
        // Check that retry-specific code IS present
        assert!(tokens.contains("__max_retries"));
        assert!(tokens.contains("__base_delay_ms"));
    }

    #[test]
    fn test_no_retry_wrapper_contains_cancellation_handling() {
        let fn_item: ItemFn = parse_quote! {
            async fn process_item(key: &str) -> Result<(), String> {
                Ok(())
            }
        };
        let config = DurableAttr {
            max_retries: Some(0),
            strategy: None,
            delay: None,
        };
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
        let tokens = result.unwrap().to_string();

        // Verify cancellation handling is present
        assert!(
            tokens.contains("should_cancel"),
            "Generated code should check for cancel signal"
        );
        assert!(
            tokens.contains("acknowledge_cancellation"),
            "Generated code should acknowledge cancellation to core"
        );
        assert!(
            tokens.contains("Instance cancelled"),
            "Generated code should return cancellation error"
        );

        // Verify SDK guard is dropped before acknowledge_cancellation to prevent deadlock
        assert!(
            tokens.contains("drop (__sdk_guard)"),
            "Generated code should drop SDK guard before acknowledge_cancellation"
        );

        // Verify pause handling is present
        assert!(
            tokens.contains("should_pause"),
            "Generated code should check for pause signal"
        );
        assert!(
            tokens.contains("Instance paused"),
            "Generated code should return pause error"
        );
    }

    #[test]
    fn test_retry_wrapper_contains_cancellation_handling() {
        let fn_item: ItemFn = parse_quote! {
            async fn process_item(key: &str) -> Result<(), String> {
                Ok(())
            }
        };
        let config = DurableAttr {
            max_retries: Some(3),
            strategy: None,
            delay: Some(1000),
        };
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
        let tokens = result.unwrap().to_string();

        // Verify cancellation handling is present in retry path
        assert!(
            tokens.contains("should_cancel"),
            "Generated code should check for cancel signal"
        );
        assert!(
            tokens.contains("acknowledge_cancellation"),
            "Generated code should acknowledge cancellation to core"
        );
        assert!(
            tokens.contains("Instance cancelled"),
            "Generated code should return cancellation error"
        );

        // Verify pause handling is present
        assert!(
            tokens.contains("should_pause"),
            "Generated code should check for pause signal"
        );
        assert!(
            tokens.contains("Instance paused"),
            "Generated code should return pause error"
        );

        // Verify SDK guard is dropped before acknowledge_cancellation to prevent deadlock
        assert!(
            tokens.contains("drop (__sdk_guard)"),
            "Generated code should drop SDK guard before acknowledge_cancellation"
        );
    }

    #[test]
    fn test_cancellation_returns_error_not_just_logs() {
        let fn_item: ItemFn = parse_quote! {
            async fn my_function(key: &str) -> Result<i32, String> {
                Ok(42)
            }
        };
        let config = DurableAttr::default();
        let result = generate_durable_wrapper(fn_item, config);
        assert!(result.is_ok());
        let tokens = result.unwrap().to_string();

        // The old buggy behavior just logged "should exit" without returning
        // Verify we now have return statements after cancellation detection
        assert!(
            !tokens.contains("should exit"),
            "Should not contain old 'should exit' message"
        );
        assert!(
            tokens.contains("exiting"),
            "Should contain new 'exiting' message"
        );

        // Verify we return Err, not just log
        // The pattern is: if should_cancel() { ... return Err(...) }
        assert!(
            tokens.contains("return Err"),
            "Should return error on cancellation, not just log"
        );
    }

    /// Test that verifies the deadlock fix: drop(__sdk_guard) must appear BEFORE
    /// acknowledge_cancellation() in the generated code.
    ///
    /// Background: The deadlock occurred because:
    /// 1. __sdk_guard = __sdk.lock().await holds the SDK mutex
    /// 2. acknowledge_cancellation() tries to acquire the same mutex via SDK_INSTANCE.get()
    /// 3. This creates a self-deadlock
    ///
    /// The fix is to drop the guard before calling acknowledge_cancellation().
    /// This test verifies the ORDER of operations, not just their presence.
    #[test]
    fn test_deadlock_fix_drop_guard_before_acknowledge() {
        // Test both no-retry and retry paths
        for (max_retries, path_name) in [(Some(0), "no-retry"), (Some(3), "retry")] {
            let fn_item: ItemFn = parse_quote! {
                async fn process_item(key: &str) -> Result<(), String> {
                    Ok(())
                }
            };
            let config = DurableAttr {
                max_retries,
                strategy: None,
                delay: if max_retries == Some(3) {
                    Some(1000)
                } else {
                    None
                },
            };
            let result = generate_durable_wrapper(fn_item, config);
            assert!(result.is_ok(), "{} path should compile", path_name);
            let tokens = result.unwrap().to_string();

            // Find positions of key statements
            let drop_pos = tokens.find("drop (__sdk_guard)");
            let ack_pos = tokens.find("acknowledge_cancellation");

            assert!(
                drop_pos.is_some(),
                "{} path: drop(__sdk_guard) should be present",
                path_name
            );
            assert!(
                ack_pos.is_some(),
                "{} path: acknowledge_cancellation should be present",
                path_name
            );

            // CRITICAL: drop must come BEFORE acknowledge_cancellation
            assert!(
                drop_pos.unwrap() < ack_pos.unwrap(),
                "{} path: drop(__sdk_guard) (pos {}) must appear BEFORE acknowledge_cancellation (pos {}) to prevent deadlock",
                path_name,
                drop_pos.unwrap(),
                ack_pos.unwrap()
            );
        }
    }
}
