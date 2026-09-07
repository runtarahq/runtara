use super::*;

#[derive(FromMeta)]
struct AgentComponentArgs {
    agent: String,
    capabilities: syn::ExprArray,
    #[darling(default)]
    decode_input: Option<syn::Path>,
}

/// Generate the standard callback export and dispatch for a built-in Agent.
/// Capability IDs and invocation adapters come from `#[capability]`; the list
/// contains Rust function names, not a second copy of the wire IDs. This owns
/// no tasks and does not transform blocking capability bodies into async I/O.
pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let args = match darling::ast::NestedMeta::parse_meta_list(input.into()) {
        Ok(args) => args,
        Err(error) => return error.into_compile_error().into(),
    };
    let args = match AgentComponentArgs::from_list(&args) {
        Ok(args) => args,
        Err(error) => return error.write_errors().into(),
    };
    if args.agent.is_empty()
        || !args
            .agent
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || args.agent.starts_with('-')
        || args.agent.ends_with('-')
        || args.agent.contains("--")
    {
        return syn::Error::new_spanned(
            &args.capabilities,
            "agent must be a kebab-case identifier",
        )
        .into_compile_error()
        .into();
    }
    let agent = &args.agent;
    let interface = format_ident!("agent_{}", agent.replace('-', "_"));
    let world = format!("runtara:agent-{agent}/agent");
    let export = format!("export:runtara:agent-{agent}/capabilities@0.4.0#invoke");
    let decode_input = args
        .decode_input
        .map(|path| quote! { #path(&input) })
        .unwrap_or_else(|| quote! { serde_json::from_slice(&input) });
    let mut arms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for capability in &args.capabilities.elems {
        let syn::Expr::Path(path) = capability else {
            return syn::Error::new_spanned(capability, "expected a capability function name")
                .into_compile_error()
                .into();
        };
        let Some(name) = path.path.get_ident() else {
            return syn::Error::new_spanned(capability, "capabilities must be in this module")
                .into_compile_error()
                .into();
        };
        if !seen.insert(name.to_string()) {
            return syn::Error::new_spanned(capability, "duplicate capability function")
                .into_compile_error()
                .into();
        }
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let invoke = format_ident!("__invoke_{name}");
        arms.push(quote! { #id => #invoke(value).await, });
    }
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[allow(warnings)]
        mod bindings {
            wit_bindgen::generate!({
                path: ["../../runtara-agent-wit/wit", "wit"],
                world: #world,
                async: [#export],
                generate_all,
            });
        }
        #[cfg(target_arch = "wasm32")]
        use bindings::exports::runtara::#interface::capabilities::ErrorInfo;
        #[cfg(target_arch = "wasm32")]
        struct Component;
        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::#interface::capabilities::Guest for Component {
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
                let value: serde_json::Value = #decode_input.map_err(bad_json)?;
                let result = match capability_id.as_str() {
                    #(#arms)*
                    other => return Err(ErrorInfo {
                        code: "UNKNOWN_CAPABILITY".into(),
                        message: format!("{} agent has no capability `{other}`", #agent),
                        category: "permanent".into(), severity: "error".into(),
                        retryable: false, retry_after_ms: None, attributes: None,
                    }),
                };
                result.map_err(error_string_to_error_info)
                    .and_then(|value| serde_json::to_vec(&value).map_err(bad_json))
            }
        }
        #[cfg(target_arch = "wasm32")]
        fn bad_json(e: serde_json::Error) -> ErrorInfo {
            ErrorInfo {
                code: "INPUT_DESERIALIZATION_ERROR".into(),
                message: e.to_string(),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }
        }

        /// The `#[capability]` macro packages each error as a JSON-string with
        /// `{ code, message, category, severity, ... }`. Parse it back into a typed
        /// `ErrorInfo` for the WIT result.
        #[cfg(target_arch = "wasm32")]
        fn error_string_to_error_info(s: String) -> ErrorInfo {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
                let category = value
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("permanent")
                    .to_string();
                let retryable = value
                    .get("retryable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(|| category == "transient");
                ErrorInfo {
                    code: value
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("CAPABILITY_ERROR")
                        .into(),
                    message: value
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&s)
                        .into(),
                    category,
                    severity: value
                        .get("severity")
                        .and_then(|v| v.as_str())
                        .unwrap_or("error")
                        .into(),
                    retryable,
                    retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
                    attributes: value.get("attributes").map(|v| v.to_string()),
                }
            } else {
                ErrorInfo {
                    code: "CAPABILITY_ERROR".into(),
                    message: s,
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        bindings::export!(Component with_types_in bindings);
    }
    .into()
}
