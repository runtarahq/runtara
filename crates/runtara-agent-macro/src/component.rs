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
    /// Capabilities that suspend (`#[capability(suspends = true)]`). The one
    /// `capabilities.invoke` answers them with their continuation and may
    /// return `suspended`.
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
    expand_tokens(input.into()).into()
}

/// [`expand`] over `proc_macro2` tokens, so unit tests can inspect the
/// generated glue.
fn expand_tokens(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let args = match darling::ast::NestedMeta::parse_meta_list(input) {
        Ok(args) => args,
        Err(error) => return error.into_compile_error(),
    };
    let args = match AgentComponentArgs::from_list(&args) {
        Ok(args) => args,
        Err(error) => return error.write_errors(),
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
        .into_compile_error();
    }
    if args.trusted && (args.control_executor || args.suspending.is_some()) {
        return syn::Error::new_spanned(
            &args.capabilities,
            "a trusted agent can neither suspend nor be the control executor",
        )
        .into_compile_error();
    }
    if args.control_executor && args.suspending.is_some() {
        return syn::Error::new_spanned(
            &args.capabilities,
            "the control executor cannot declare suspending capabilities",
        )
        .into_compile_error();
    }
    let agent = &args.agent;
    let interface = format_ident!("agent_{}", agent.replace('-', "_"));
    let world = format!("runtara:agent-{agent}/agent");
    let version = runtara_wit::VERSION;
    let export = format!("export:runtara:agent-{agent}/capabilities@{version}#invoke");
    // The per-agent package is generated inline from the one generator the
    // compiler also uses; `path` only supplies the shared packages it names,
    // as absolute directories inside `runtara-wit`.
    let shape = runtara_wit::AgentShape {
        suspendable: args.suspending.is_some(),
        trusted: args.trusted,
        control: args.control_executor,
        ..Default::default()
    };
    let package = runtara_wit::agent_package(agent, shape);
    let mut dirs = vec![format!("{}/agent", runtara_wit::WIT_DIR)];
    if args.trusted {
        dirs.push(format!("{}/trusted", runtara_wit::WIT_DIR));
    }
    if args.control_executor {
        dirs.push(format!("{}/control", runtara_wit::WIT_DIR));
    }
    let wit_paths = quote! { path: [#(#dirs),*], inline: #package };
    let trusted_export = format!("export:{}#invoke", runtara_wit::trusted::EXECUTION);
    let async_exports = if args.trusted {
        quote! { [#export, #trusted_export] }
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
                .into_compile_error();
        };
        // A capability may live in a child module (`downloads::download_file`);
        // its generated ID and adapter sit beside it in that module.
        let segments = &path.path.segments;
        let Some(last) = segments.last() else {
            return syn::Error::new_spanned(capability, "expected a capability function name")
                .into_compile_error();
        };
        if path.path.leading_colon.is_some()
            || segments.iter().any(|segment| !segment.arguments.is_none())
        {
            return syn::Error::new_spanned(capability, "capabilities must be in this crate")
                .into_compile_error();
        }
        let name = &last.ident;
        let key = quote!(#path).to_string();
        if !seen.insert(key) {
            return syn::Error::new_spanned(capability, "duplicate capability function")
                .into_compile_error();
        }
        let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let invoke = format_ident!("__invoke_{name}");
        arms.push(quote! { #(#module::)* #id => #(#module::)* #invoke(value).await, });
        listed.push((quote!(#path).to_string(), path.clone()));
    }
    if args.control_executor {
        return control_executor(&args, agent, &interface, &world, &wit_paths, &listed);
    }
    let (suspension, suspending_dispatch) = match ordinary_suspension(&args, agent, &listed) {
        Ok(tokens) => tokens,
        Err(error) => return error.into_compile_error(),
    };
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[allow(warnings)]
        mod bindings {
            wit_bindgen::generate!({
                #wit_paths,
                world: #world,
                async: #async_exports,
                generate_all,
            });
        }
        #[cfg(target_arch = "wasm32")]
        use bindings::exports::runtara::#interface::capabilities::{ErrorInfo, Outcome as __Outcome};
        #[cfg(target_arch = "wasm32")]
        struct Component;
        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::#interface::capabilities::Guest for Component {
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<__Outcome, ErrorInfo> {
                let value: serde_json::Value = #decode_input.map_err(bad_json)?;
                #suspending_dispatch
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
                    .map(__Outcome::Completed)
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
        #suspension

        #[cfg(target_arch = "wasm32")]
        bindings::export!(Component with_types_in bindings);
    }
}

/// The `suspending` list, checked against `capabilities`: each entry must be
/// listed there, once. Keys are the capability paths as written.
fn suspending_set(
    args: &AgentComponentArgs,
    listed: &[(String, syn::ExprPath)],
) -> syn::Result<std::collections::HashSet<String>> {
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
    Ok(suspending)
}

/// Compile-time checks that `suspending` matches `#[capability(suspends)]`.
fn suspension_assertions(
    listed: &[(String, syn::ExprPath)],
    suspending: &std::collections::HashSet<String>,
) -> Vec<proc_macro2::TokenStream> {
    listed
        .iter()
        .map(|(key, path)| {
            let segments = &path.path.segments;
            let name = &segments.last().expect("validated capability path").ident;
            let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
            let suspends =
                format_ident!("__CAPABILITY_SUSPENDS_{}", name.to_string().to_uppercase());
            let is_suspending = suspending.contains(key);
            let message = if is_suspending {
                format!(
                    "`{key}` is listed in `suspending` but is not #[capability(suspends = true)]"
                )
            } else {
                format!(
                    "`{key}` is #[capability(suspends = true)] but is not listed in `suspending`"
                )
            };
            quote! {
                const _: () = assert!(#(#module::)* #suspends == #is_suspending, #message);
            }
        })
        .collect()
}

/// Glue for an ordinary agent that declares suspending capabilities, and the
/// dispatch `capabilities.invoke` runs first: a suspending capability reads
/// the continuation of the operation it runs in through
/// `runtara:agent/continuation`, which the host answers from its own execution
/// state, and may answer `suspended`. Both empty without `suspending`.
fn ordinary_suspension(
    args: &AgentComponentArgs,
    agent: &str,
    listed: &[(String, syn::ExprPath)],
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    if args.suspending.is_none() {
        return Ok((
            proc_macro2::TokenStream::new(),
            proc_macro2::TokenStream::new(),
        ));
    }
    let suspending = suspending_set(args, listed)?;
    let assertions = suspension_assertions(listed, &suspending);
    let mut ids = Vec::new();
    let mut arms = Vec::new();
    for (key, path) in listed {
        if !suspending.contains(key) {
            continue;
        }
        let segments = &path.path.segments;
        let name = &segments.last().expect("validated capability path").ident;
        let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let suspend = format_ident!("__suspend_{name}");
        ids.push(quote! { #(#module::)* #id });
        arms.push(quote! {
            #(#module::)* #id => #(#module::)* #suspend(value, &context)
                .await
                .map(__suspendable_to_outcome),
        });
    }
    // A suspending capability answers through the same `capabilities.invoke`
    // as every other one, with the continuation of the operation it runs in.
    let dispatch = quote! {
        if [#(#ids),*].contains(&capability_id.as_str()) {
            return __invoke_suspending(&capability_id, value).await;
        }
    };
    let glue = quote! {
        #(#assertions)*

        // Aliased: the agent crate may import the Rust-side `Wake` / `Suspendable`.
        #[cfg(target_arch = "wasm32")]
        use bindings::runtara::agent::types::{Suspension as __Suspension, Wake as __Wake};

        #[cfg(target_arch = "wasm32")]
        async fn __invoke_suspending(
            capability_id: &str,
            value: serde_json::Value,
        ) -> Result<__Outcome, ErrorInfo> {
            // The host derives the operation from its own state, never from
            // this call's arguments.
            let context = runtara_agent_suspension::SuspendContext::new(
                bindings::runtara::agent::continuation::continuation(),
            );
            let result = match capability_id {
                #(#arms)*
                other => return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("{} agent has no suspending capability `{other}`", #agent),
                    category: "permanent".into(), severity: "error".into(),
                    retryable: false, retry_after_ms: None, attributes: None,
                }),
            };
            result.map_err(error_string_to_error_info)
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
    };
    Ok((glue, dispatch))
}

/// Glue for the control agent. The composed copy never runs a capability body:
/// `capabilities.invoke` forwards to the host executor, which runs
/// `execution.invoke` on approved bytes in a fresh store. No control
/// capability suspends; that is checked against the metadata at compile time.
fn control_executor(
    args: &AgentComponentArgs,
    agent: &str,
    interface: &syn::Ident,
    world: &str,
    wit_paths: &proc_macro2::TokenStream,
    listed: &[(String, syn::ExprPath)],
) -> proc_macro2::TokenStream {
    let mut assertions = Vec::new();
    let mut execution_arms = Vec::new();
    for (key, path) in listed {
        let segments = &path.path.segments;
        let name = &segments.last().expect("validated capability path").ident;
        let module: Vec<_> = segments.iter().take(segments.len() - 1).collect();
        let id = format_ident!("__CAPABILITY_ID_{}", name.to_string().to_uppercase());
        let invoke = format_ident!("__invoke_{name}");
        let suspends = format_ident!("__CAPABILITY_SUSPENDS_{}", name.to_string().to_uppercase());
        let message = format!(
            "`{key}` is #[capability(suspends = true)], and no control capability may suspend"
        );
        assertions.push(quote! {
            const _: () = assert!(!#(#module::)* #suspends, #message);
        });
        execution_arms.push(quote! {
            #(#module::)* #id => #(#module::)* #invoke(value)
                .await
                .and_then(|value| serde_json::to_vec(&value).map_err(|e| e.to_string())),
        });
    }
    let decode_input = args
        .decode_input
        .as_ref()
        .map(|path| quote! { #path(&input) })
        .unwrap_or_else(|| quote! { serde_json::from_slice(&input) });
    quote! {
        #(#assertions)*

        #[cfg(target_arch = "wasm32")]
        #[allow(warnings)]
        mod bindings {
            wit_bindgen::generate!({
                #wit_paths,
                world: #world,
                async: true,
                generate_all,
            });
        }
        #[cfg(target_arch = "wasm32")]
        use bindings::exports::runtara::#interface::capabilities::{ErrorInfo, Outcome as __Outcome};
        #[cfg(target_arch = "wasm32")]
        struct Component;

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
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<__Outcome, ErrorInfo> {
                bindings::runtara::control::executor::invoke(capability_id, input)
                    .await
                    .map(__Outcome::Completed)
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl bindings::exports::runtara::control::execution::Guest for Component {
            async fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
                let value: serde_json::Value = #decode_input.map_err(|e| {
                    __control_error("INPUT_DESERIALIZATION_ERROR", e.to_string())
                })?;
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
    }
}

#[cfg(test)]
mod tests {
    use super::expand_tokens;
    use quote::quote;

    /// Whitespace-free rendering, so assertions do not depend on how
    /// `proc_macro2` spaces tokens.
    fn expand(input: proc_macro2::TokenStream) -> String {
        expand_tokens(input)
            .to_string()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    #[test]
    fn the_control_executor_forwards_capabilities_and_runs_execution() {
        let glue = expand(quote! {
            agent = "control",
            control_executor = true,
            capabilities = [get, commands::cancel],
        });
        // The composed copy runs no capability body: `capabilities` forwards
        // to the host executor.
        assert_eq!(
            glue.matches("bindings::runtara::control::executor::invoke(capability_id,input)")
                .count(),
            1,
            "{glue}"
        );
        for export in [
            "implbindings::exports::runtara::agent_control::capabilities::GuestforComponent",
            "implbindings::exports::runtara::control::execution::GuestforComponent",
        ] {
            assert!(glue.contains(export), "missing {export}");
        }
        // Execution is a plain call: no suspension glue, no continuation.
        assert!(!glue.contains("suspendable"), "{glue}");
        assert!(!glue.contains("agent_suspension"), "{glue}");
        assert!(!glue.contains("continuation"), "{glue}");
        assert!(glue.contains("__invoke_get(value)"));
        assert!(glue.contains("commands::__invoke_cancel(value)"));
        // No control capability may suspend, checked at compile time.
        assert!(glue.contains("const_:()=assert!(!__CAPABILITY_SUSPENDS_GET,"));
        assert!(glue.contains("const_:()=assert!(!commands::__CAPABILITY_SUSPENDS_CANCEL,"));
        assert!(glue.contains(r#"world:"runtara:agent-control/agent""#));
        let agent_dir = format!("{}/agent", runtara_wit::WIT_DIR);
        let control_dir = format!("{}/control", runtara_wit::WIT_DIR);
        assert!(
            glue.contains(&format!(r#"path:["{agent_dir}","{control_dir}"]"#)),
            "{glue}"
        );
        assert!(
            glue.contains("inline:\"packageruntara:agent-control@"),
            "{glue}"
        );
    }

    #[test]
    fn an_ordinary_suspending_agent_answers_through_capabilities_with_its_context() {
        let glue = expand(quote! {
            agent = "suspend-probe",
            capabilities = [plain, waits::pause],
            suspending = [waits::pause],
        });
        // One async export; the suspending capability is dispatched from it.
        assert!(
            glue.contains(&format!(
                r#"async:["export:runtara:agent-suspend-probe/capabilities@{v}#invoke"]"#,
                v = runtara_wit::VERSION
            )),
            "{glue}"
        );
        assert!(!glue.contains("interfacesuspendable"), "{glue}");
        assert!(glue.contains("importruntara:agent/continuation@"), "{glue}");
        assert!(
            glue.contains("return__invoke_suspending(&capability_id,value).await;"),
            "{glue}"
        );
        // The continuation comes from the host context, never from arguments.
        assert!(glue.contains(
            "runtara_agent_suspension::SuspendContext::new(bindings::runtara::agent::continuation::continuation(),)"
        ), "{glue}");
        assert!(glue.contains("waits::__suspend_pause(value,&context)"));
        // The plain match keeps every capability's adapter; the suspending one
        // is never reached because the dispatch above returns first.
        assert!(glue.contains("waits::__invoke_pause(value)"));
        assert!(glue.contains("__invoke_plain(value)"));
        assert!(!glue.contains("__suspend_plain"));
        assert!(glue.contains("const_:()=assert!(waits::__CAPABILITY_SUSPENDS_PAUSE==true"));
        assert!(glue.contains("const_:()=assert!(__CAPABILITY_SUSPENDS_PLAIN==false"));
        // No control forwarding outside the control agent.
        assert!(!glue.contains("control::executor"));
    }

    #[test]
    fn an_agent_without_suspending_capabilities_is_unchanged() {
        let glue = expand(quote! { agent = "plain", capabilities = [get] });
        assert!(!glue.contains("suspendable"));
        assert!(!glue.contains("agent_suspension"));
        assert!(glue.contains(&format!(r#"path:["{}/agent"]"#, runtara_wit::WIT_DIR)));
    }

    #[test]
    fn misdeclared_suspension_lists_are_rejected() {
        for (input, message) in [
            (
                quote! { agent = "probe", capabilities = [get], suspending = [wait] },
                "a suspending capability must also be listed in `capabilities`",
            ),
            (
                quote! {
                    agent = "probe",
                    capabilities = [wait], suspending = [wait, wait]
                },
                "duplicate suspending capability",
            ),
            (
                quote! {
                    agent = "control", control_executor = true,
                    capabilities = [get, wait], suspending = [wait]
                },
                "the control executor cannot declare suspending capabilities",
            ),
            (
                quote! {
                    agent = "signer", trusted = true, control_executor = true,
                    capabilities = [sign]
                },
                "a trusted agent can neither suspend nor be the control executor",
            ),
        ] {
            let tokens = expand_tokens(input).to_string();
            assert!(
                tokens.contains("compile_error") && tokens.contains(message),
                "expected `{message}` in {tokens}"
            );
        }
    }
}
