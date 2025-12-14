// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Proc macros for runtara-sdk.
//!
//! Provides the `#[durable]` attribute macro for transparent durability.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, ReturnType, Type, parse_macro_input, spanned::Spanned};

/// Makes an async function durable by wrapping it with checkpoint-based caching.
///
/// The macro automatically:
/// - Checks for existing checkpoint before execution
/// - Returns cached result if checkpoint exists
/// - Executes function and saves result as checkpoint if no cache
///
/// # Requirements
///
/// - Function must be async
/// - **First parameter is the idempotency key** (any type that implements `Display`)
/// - Function must return `Result<T, E>` where `T: Serialize + DeserializeOwned`
/// - SDK must be registered via `RuntaraSdk::init()` before calling
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::durable;
///
/// #[durable]
/// pub async fn fetch_order(key: &str, order_id: &str) -> Result<Order, OrderError> {
///     // The key determines caching - same key = same cached result
///     db.fetch_order(order_id).await
/// }
///
/// // Usage:
/// fetch_order("order-123", "123").await
/// ```
#[proc_macro_attribute]
pub fn durable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    match generate_durable_wrapper(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate_durable_wrapper(input: ItemFn) -> syn::Result<TokenStream2> {
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
            let __result: Result<_, _> = (|| async #block)().await;

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
                                    // Let the caller handle cancellation via signal polling
                                } else if checkpoint_result.should_pause() {
                                    ::tracing::info!(
                                        function = #fn_name_str,
                                        "Pause signal pending - instance should exit after returning"
                                    );
                                    // Let the caller handle pause via signal polling
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
