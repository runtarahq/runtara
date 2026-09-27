use super::*;

#[derive(FromMeta)]
struct AgentComponentArgs {
    agent: String,
    #[darling(default)]
    trusted: bool,
    /// The built-in control agent: its composed copy forwards every call to
    /// `runtara:control/executor`, and the host runs `runtara:control/execution`
    /// on approved bytes in a fresh store where `runtara:control/api` is real.
    #[darling(default)]
    control_executor: bool,
    /// Capabilities that suspend (`#[capability(suspends = true)]`). They are
    /// exported through `suspendable.invoke`; plain `capabilities.invoke`
    /// refuses them.
    #[darling(default)]
    suspending: Option<syn::ExprArray>,
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
    if args.trusted && (args.control_executor || args.suspending.is_some()) {
        return syn::Error::new_spanned(
            &args.capabilities,
            "a trusted agent can neither suspend nor be the control executor",
        )
        .into_compile_error()
        .into();
    }
    if args.suspending.is_some() && !args.control_executor {
        // Ordinary suspending agents read their continuation through
        // `runtara:agent-suspension/context`; that glue is not generated yet.
        return syn::Error::new_spanned(
            &args.capabilities,
            "`suspending` is supported only with `control_executor = true`",
        )
        .into_compile_error()
        .into();
    }
    let agent = &args.agent;
    let interface = format_ident!("agent_{}", agent.replace('-', "_"));
    let world = format!("runtara:agent-{agent}/agent");
    let export = format!("export:runtara:agent-{agent}/capabilities@0.4.0#invoke");
    let wit_paths = if args.trusted {
        quote! { ["../../runtara-agent-wit/wit", "../../runtara-agent-trusted/wit", "wit"] }
    } else if args.control_executor {
        quote! { [
            "../../runtara-agent-wit/wit",
            "../../runtara-agent-suspension/wit",
            "../../runtara-workflow-wit/wit/control",
            "wit",
        ] }
    } else {
        quote! { ["../../runtara-agent-wit/wit", "wit"] }
    };
    let async_exports = if args.trusted {
        quote! { [#export, "export:runtara:trusted/execution@0.1.0#invoke"] }
    } else if args.control_executor {
        // Every function in the control world is async-typed; the forwarding
        // imports are awaited, never blocked on.
        quote! { true }
    } else {
        quote! { [#export] }
    };
    let decode_input = args
        .decode_input
        .as_ref()
        .map(|path| quote! { #path(&input) })
        .unwrap_or_else(|| quote! { serde_json::from_slice(&input) });
    let mut arms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // `(module path, function name)` of every listed capability, for the
    // control-executor glue and the suspension metadata assertions.
    let mut listed = Vec::new();
    for capability in &args.capabilities.elems {
        let syn::Expr::Path(path) = capability else {
            return syn::Error::new_spanned(capability, "expected a capability function name")
                .into_compile_error()
                .into();
        };
        // A capability may live in a child module (`downloads::download_file`);
        // its generated ID and adapter sit beside it in that module.
        let segments = &path.path.segments;
        let Some(last) = segments.last() else {
            return syn::Error::new_spanned(capability, "expected a capability function name")
                .into_compile_error()
                .into();
        };
        if path.path.leading_colon.is_some()
            || segments.iter().any(|segment| !segment.arguments.is_none())
        {
            return syn::Error::new_spanned(capability, "capabilities must be in this crate")
                .into_compile_error()
                .into();
        }
        let name = &last.ident;
        let key = quote!(#path).to_string();
        if !seen.insert(key) {
            return syn::Error::new_spanned(capability, "duplicate capability function")
                .into_compile_error()
                .into();
        }
        let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let invoke = format_ident!("__invoke_{name}");
        arms.push(quote! { #(#module::)* #id => #(#module::)* #invoke(value).await, });
        listed.push((quote!(#path).to_string(), path.clone()));
    }
    if args.control_executor {
        return control_executor(&args, agent, &interface, &world, &wit_paths, &listed)
            .unwrap_or_else(syn::Error::into_compile_error)
            .into();
    }
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[allow(warnings)]
        mod bindings {
            wit_bindgen::generate!({
                path: #wit_paths,
                world: #world,
                async: #async_exports,
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

/// Glue for the control agent. The composed copy never runs a capability body:
/// `capabilities.invoke` and `suspendable.invoke` forward to the host executor,
/// which runs `execution.invoke` on approved bytes in a fresh store and supplies
/// the caller operation's continuation.
fn control_executor(
    args: &AgentComponentArgs,
    agent: &str,
    interface: &syn::Ident,
    world: &str,
    wit_paths: &proc_macro2::TokenStream,
    listed: &[(String, syn::ExprPath)],
) -> syn::Result<proc_macro2::TokenStream> {
    let mut suspending = std::collections::HashSet::new();
    if let Some(list) = &args.suspending {
        for capability in &list.elems {
            let syn::Expr::Path(path) = capability else {
                return Err(syn::Error::new_spanned(
                    capability,
                    "expected a capability function name",
                ));
            };
            let key = quote!(#path).to_string();
            if !listed.iter().any(|(listed, _)| *listed == key) {
                return Err(syn::Error::new_spanned(
                    capability,
                    "a suspending capability must also be listed in `capabilities`",
                ));
            }
            if !suspending.insert(key) {
                return Err(syn::Error::new_spanned(
                    capability,
                    "duplicate suspending capability",
                ));
            }
        }
    }
    let mut assertions = Vec::new();
    let mut execution_arms = Vec::new();
    let mut suspending_ids = Vec::new();
    for (key, path) in listed {
        let segments = &path.path.segments;
        let name = &segments.last().expect("validated capability path").ident;
        let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let suspends = format_ident!("__CAPABILITY_SUSPENDS_{}", name.to_string().to_uppercase());
        let is_suspending = suspending.contains(key);
        let message = if is_suspending {
            format!("`{key}` is listed in `suspending` but is not #[capability(suspends = true)]")
        } else {
            format!("`{key}` is #[capability(suspends = true)] but is not listed in `suspending`")
        };
        assertions.push(quote! {
            const _: () = assert!(#(#module::)* #suspends == #is_suspending, #message);
        });
        if is_suspending {
            let suspend = format_ident!("__suspend_{name}");
            suspending_ids.push(quote! { #(#module::)* #id });
            execution_arms.push(quote! {
                #(#module::)* #id => #(#module::)* #suspend(value, &context)
                    .await
                    .map(__suspendable_to_outcome),
            });
        } else {
            let invoke = format_ident!("__invoke_{name}");
            execution_arms.push(quote! {
                #(#module::)* #id => #(#module::)* #invoke(value)
                    .await
                    .and_then(|value| serde_json::to_vec(&value).map_err(|e| e.to_string()))
                    .map(__Outcome::Completed),
            });
        }
    }
    let decode_input = args
        .decode_input
        .as_ref()
        .map(|path| quote! { #path(&input) })
        .unwrap_or_else(|| quote! { serde_json::from_slice(&input) });
    Ok(quote! {
        #(#assertions)*

        #[cfg(target_arch = "wasm32")]
        #[allow(warnings)]
        mod bindings {
            wit_bindgen::generate!({
                path: #wit_paths,
                world: #world,
                async: true,
                generate_all,
            });
        }
        #[cfg(target_arch = "wasm32")]
        use bindings::exports::runtara::#interface::capabilities::ErrorInfo;
        // Aliased: the agent crate may import the Rust-side `Wake` / `Suspendable`.
        #[cfg(target_arch = "wasm32")]
        use bindings::runtara::agent_suspension::types::{
            Outcome as __Outcome, Suspension as __Suspension, Wake as __Wake,
        };
        #[cfg(target_arch = "wasm32")]
        struct Component;

        /// Capabilities whose calls must go through `suspendable.invoke`.
        #[cfg(target_arch = "wasm32")]
        fn __is_suspending(capability_id: &str) -> bool {
            [#(#suspending_ids),*].contains(&capability_id)
        }

        #[cfg(target_arch = "wasm32")]
        fn __control_error(code: &str, message: String) -> ErrorInfo {
            ErrorInfo {
                code: code.into(),
                message,
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::#interface::capabilities::Guest for Component {
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
                if __is_suspending(&capability_id) {
                    return Err(__control_error(
                        runtara_agent_suspension::SUSPENSION_UNSUPPORTED,
                        format!("{capability_id} suspends and can only run as a durable workflow step"),
                    ));
                }
                match bindings::runtara::control::executor::invoke(capability_id, input).await? {
                    __Outcome::Completed(output) => Ok(output),
                    __Outcome::Suspended(_) => Err(__control_error(
                        "AGENT_UNEXPECTED_SUSPEND",
                        "a non-suspending capability returned a suspension".into(),
                    )),
                }
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::#interface::suspendable::Guest for Component {
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<__Outcome, ErrorInfo> {
                if !__is_suspending(&capability_id) {
                    return Err(__control_error(
                        "UNKNOWN_CAPABILITY",
                        format!("{} agent has no suspending capability `{capability_id}`", #agent),
                    ));
                }
                bindings::runtara::control::executor::invoke(capability_id, input).await
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::control::execution::Guest for Component {
            async fn invoke(
                capability_id: String,
                input: Vec<u8>,
                continuation: Option<Vec<u8>>,
            ) -> Result<__Outcome, ErrorInfo> {
                let value: serde_json::Value = #decode_input.map_err(|e| {
                    __control_error("INPUT_DESERIALIZATION_ERROR", e.to_string())
                })?;
                let context = runtara_agent_suspension::SuspendContext::new(continuation);
                let _ = &context;
                let result = match capability_id.as_str() {
                    #(#execution_arms)*
                    other => return Err(__control_error(
                        "UNKNOWN_CAPABILITY",
                        format!("{} agent has no capability `{other}`", #agent),
                    )),
                };
                result.map_err(error_string_to_error_info)
            }
        }

        #[cfg(target_arch = "wasm32")]
        fn __suspendable_to_outcome(
            result: runtara_agent_suspension::Suspendable<serde_json::Value>,
        ) -> __Outcome {
            match result {
                runtara_agent_suspension::Suspendable::Completed(value) => {
                    // A `serde_json::Value` always serializes.
                    __Outcome::Completed(serde_json::to_vec(&value).unwrap_or_default())
                }
                runtara_agent_suspension::Suspendable::Suspended { wakes, state } => {
                    __Outcome::Suspended(__Suspension {
                        wakes: wakes
                            .into_iter()
                            .map(|wake| match wake {
                                runtara_agent_suspension::Wake::At(at) => __Wake::At(at),
                                runtara_agent_suspension::Wake::Instances(id) => {
                                    __Wake::Instances(id)
                                }
                            })
                            .collect(),
                        state,
                    })
                }
            }
        }

        /// Parse a `#[capability]` JSON error envelope into `ErrorInfo`.
        #[cfg(target_arch = "wasm32")]
        fn error_string_to_error_info(s: String) -> ErrorInfo {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) else {
                return __control_error("CAPABILITY_ERROR", s);
            };
            let field = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let category = field("category").unwrap_or_else(|| "permanent".into());
            ErrorInfo {
                code: field("code").unwrap_or_else(|| "CAPABILITY_ERROR".into()),
                message: field("message").unwrap_or_else(|| s.clone()),
                retryable: value
                    .get("retryable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(category == "transient"),
                category,
                severity: field("severity").unwrap_or_else(|| "error".into()),
                retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
                attributes: value.get("attributes").map(|v| v.to_string()),
            }
        }

        #[cfg(target_arch = "wasm32")]
        bindings::export!(Component with_types_in bindings);
    })
}
