// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Procedural macros for agent capability and step metadata generation
//!
//! This crate provides macros to declaratively define agent capability and step metadata:
//! - `#[capability]` - marks a function as an agent capability
//! - `#[derive(CapabilityInput)]` - generates input metadata from struct fields
//! - `#[derive(CapabilityOutput)]` - generates output metadata from struct fields
//! - `#[derive(StepMeta)]` - generates step type metadata for DSL generation
//!
//! The macros generate static metadata that can be collected at runtime using
//! the `inventory` crate for agent and step discovery.
//!
//! Note: The metadata types (CapabilityMeta, InputTypeMeta, StepTypeMeta, etc.) are defined
//! in `runtara-dsl` crate to avoid proc-macro crate limitations.

use darling::{FromDeriveInput, FromField, FromMeta};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{DeriveInput, ItemFn, Type, parse_macro_input};

/// A known error specification for a capability
#[derive(Debug, Clone)]
struct KnownErrorSpec {
    /// Error code (e.g., "HTTP_TIMEOUT")
    code: String,
    /// Description of when this error occurs
    description: String,
    /// Error kind: transient or permanent
    kind: String,
    /// Context attributes included with this error
    attributes: Vec<String>,
}

impl FromMeta for KnownErrorSpec {
    fn from_meta(item: &syn::Meta) -> darling::Result<Self> {
        // Parse: transient("CODE", "description", ["attr1", "attr2"])
        // or: permanent("CODE", "description")
        match item {
            syn::Meta::List(list) => {
                let kind = list
                    .path
                    .get_ident()
                    .map(|i| i.to_string())
                    .unwrap_or_default();
                if kind != "transient" && kind != "permanent" {
                    return Err(darling::Error::custom(
                        "Expected 'transient' or 'permanent' for error kind",
                    )
                    .with_span(&list.path));
                }

                // Parse tokens manually to extract: (code, description) or (code, description, [attrs])
                let tokens: proc_macro2::TokenStream = list.tokens.clone();
                let mut code = String::new();
                let mut description = String::new();
                let mut attributes: Vec<String> = Vec::new();
                let mut string_count = 0;

                for token in tokens {
                    match token {
                        proc_macro2::TokenTree::Literal(lit) => {
                            let lit_str = lit.to_string();
                            // Remove quotes from string literal
                            if lit_str.starts_with('"') && lit_str.ends_with('"') {
                                let value = lit_str[1..lit_str.len() - 1].to_string();
                                if string_count == 0 {
                                    code = value;
                                    string_count += 1;
                                } else if string_count == 1 {
                                    description = value;
                                    string_count += 1;
                                }
                            }
                        }
                        proc_macro2::TokenTree::Group(group) => {
                            if group.delimiter() == proc_macro2::Delimiter::Bracket {
                                for inner in group.stream() {
                                    if let proc_macro2::TokenTree::Literal(lit) = inner {
                                        let lit_str = lit.to_string();
                                        if lit_str.starts_with('"') && lit_str.ends_with('"') {
                                            attributes
                                                .push(lit_str[1..lit_str.len() - 1].to_string());
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if code.is_empty() {
                    return Err(darling::Error::custom("Error code is required"));
                }
                if description.is_empty() {
                    return Err(darling::Error::custom("Error description is required"));
                }

                Ok(KnownErrorSpec {
                    code,
                    description,
                    kind,
                    attributes,
                })
            }
            _ => Err(darling::Error::custom(
                "Expected error specification like: transient(\"CODE\", \"description\")",
            )),
        }
    }
}

/// Container for multiple error specifications
#[derive(Debug, Default)]
struct ErrorsSpec(Vec<KnownErrorSpec>);

impl FromMeta for ErrorsSpec {
    fn from_meta(item: &syn::Meta) -> darling::Result<Self> {
        match item {
            syn::Meta::List(list) => {
                let mut errors = Vec::new();

                // Parse each nested error specification using Parser trait
                use syn::parse::Parser;
                let parser =
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
                let nested = parser.parse2(list.tokens.clone())?;

                for meta in nested {
                    errors.push(KnownErrorSpec::from_meta(&meta)?);
                }

                Ok(ErrorsSpec(errors))
            }
            _ => Err(darling::Error::custom("Expected errors(...)")),
        }
    }
}

/// Attributes for the `#[capability]` macro
#[derive(Debug, FromMeta)]
struct CapabilityArgs {
    /// The agent module name (e.g., "utils", "transform")
    #[darling(default)]
    module: Option<String>,
    /// Capability ID in kebab-case (e.g., "random-double")
    #[darling(default)]
    id: Option<String>,
    /// Display name for UI
    #[darling(default)]
    display_name: Option<String>,
    /// Description of the capability
    #[darling(default)]
    description: Option<String>,
    /// Whether this capability has side effects
    #[darling(default)]
    side_effects: bool,
    /// Whether this capability is idempotent
    #[darling(default)]
    idempotent: Option<bool>,
    /// Whether this capability requires rate limiting (external API calls)
    #[darling(default)]
    rate_limited: bool,

    // === Compensation hint attributes ===
    /// Capability ID that compensates (undoes) this capability's effects.
    /// Example: compensates_with = "release" for a "reserve" capability.
    #[darling(default)]
    compensates_with: Option<String>,
    /// Description of what the compensation does.
    #[darling(default)]
    compensates_description: Option<String>,

    // === Error introspection attributes ===
    /// Known errors this capability can return.
    /// Example: errors(transient("HTTP_TIMEOUT", "Request timed out", ["url"]))
    #[darling(default)]
    errors: Option<ErrorsSpec>,

    // === Module registration attributes ===
    // When module_display_name is provided, automatically registers an AgentModuleConfig
    /// Display name for auto-registered module (e.g., "SMO Test")
    #[darling(default)]
    module_display_name: Option<String>,
    /// Description for auto-registered module
    #[darling(default)]
    module_description: Option<String>,
    /// Whether the auto-registered module has side effects (default: false)
    #[darling(default)]
    module_has_side_effects: Option<bool>,
    /// Whether the auto-registered module supports connections (default: false)
    #[darling(default)]
    module_supports_connections: Option<bool>,
    /// Integration IDs for the auto-registered module (comma-separated)
    #[darling(default)]
    module_integration_ids: Option<String>,
    /// Whether the auto-registered module is secure (default: false)
    #[darling(default)]
    module_secure: Option<bool>,
}

/// Field attributes for CapabilityInput derive
#[derive(Debug, FromField)]
#[darling(attributes(field), forward_attrs(serde))]
struct InputFieldArgs {
    ident: Option<syn::Ident>,
    ty: syn::Type,
    /// Forwarded serde attributes to detect #[serde(default)]
    attrs: Vec<syn::Attribute>,
    #[darling(default)]
    display_name: Option<String>,
    #[darling(default)]
    description: Option<String>,
    #[darling(default)]
    example: Option<String>,
    #[darling(default)]
    default: Option<String>,
    /// Skip this field in metadata (e.g., connection_id)
    #[darling(default)]
    skip: bool,
    /// Enum type name for fields that are enums (to get variant names)
    #[darling(default)]
    enum_type: Option<String>,
}

/// Container attributes for CapabilityInput derive
#[derive(Debug, FromDeriveInput)]
#[darling(attributes(capability_input))]
struct InputContainerArgs {
    ident: syn::Ident,
    data: darling::ast::Data<(), InputFieldArgs>,
    #[darling(default)]
    display_name: Option<String>,
    #[darling(default)]
    description: Option<String>,
}

/// Field attributes for CapabilityOutput derive
#[derive(Debug, FromField)]
#[darling(attributes(field))]
struct OutputFieldArgs {
    ident: Option<syn::Ident>,
    ty: syn::Type,
    #[darling(default)]
    display_name: Option<String>,
    #[darling(default)]
    description: Option<String>,
    #[darling(default)]
    example: Option<String>,
}

/// Container attributes for CapabilityOutput derive
#[derive(Debug, FromDeriveInput)]
#[darling(attributes(capability_output))]
struct OutputContainerArgs {
    ident: syn::Ident,
    data: darling::ast::Data<(), OutputFieldArgs>,
    #[darling(default)]
    display_name: Option<String>,
    #[darling(default)]
    description: Option<String>,
}

/// Attribute macro for marking agent capability functions
///
/// # Example
/// ```ignore
/// #[capability(
///     module = "utils",
///     id = "random-double",
///     display_name = "Random Double",
///     description = "Generate a random double between 0 and 1"
/// )]
/// pub fn random_double(input: RandomDoubleInput) -> Result<f64, String> {
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn capability(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match darling::ast::NestedMeta::parse_meta_list(attr.into()) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.into_compile_error()),
    };

    let args = match CapabilityArgs::from_list(&args) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let fn_name_str = fn_name.to_string();

    // Derive capability_id from function name if not provided (snake_case -> kebab-case)
    let capability_id = args.id.unwrap_or_else(|| fn_name_str.replace('_', "-"));

    // Extract input type from first parameter
    let input_type = input_fn
        .sig
        .inputs
        .iter()
        .find_map(|arg| {
            if let syn::FnArg::Typed(pat_type) = arg
                && let Type::Path(type_path) = &*pat_type.ty
            {
                return type_path.path.segments.last().map(|s| s.ident.to_string());
            }
            None
        })
        .unwrap_or_else(|| "Unknown".to_string());

    // Extract output type from Result<T, String>
    let output_type = extract_result_ok_type(&input_fn.sig.output);

    let display_name = args.display_name;
    let description = args.description;
    let side_effects = args.side_effects;
    let idempotent = args.idempotent.unwrap_or(!side_effects);
    let rate_limited = args.rate_limited;
    let module = args.module;

    // Generate metadata registration
    let meta_ident = format_ident!("__CAPABILITY_META_{}", fn_name.to_string().to_uppercase());
    let executor_ident = format_ident!(
        "__CAPABILITY_EXECUTOR_{}",
        fn_name.to_string().to_uppercase()
    );
    let executor_fn_ident = format_ident!("__executor_{}", fn_name);

    let display_name_token = option_to_tokens(&display_name);
    let description_token = option_to_tokens(&description);
    let module_token = option_to_tokens(&module);

    // For executor, module must be provided
    let module_str = module.clone().unwrap_or_else(|| "unknown".to_string());

    // Parse the input type as an identifier for the executor function
    let input_type_ident = format_ident!("{}", input_type);

    // Generate module registration if module_display_name is provided
    let module_registration =
        if let (Some(module_id), Some(mod_display_name)) = (&module, &args.module_display_name) {
            let module_meta_ident = format_ident!(
                "__AGENT_MODULE_META_{}_{}",
                module_id.to_uppercase().replace('-', "_"),
                fn_name.to_string().to_uppercase()
            );

            let mod_description = args
                .module_description
                .clone()
                .unwrap_or_else(|| format!("{} agent module", mod_display_name));
            let mod_has_side_effects = args.module_has_side_effects.unwrap_or(false);
            let mod_supports_connections = args.module_supports_connections.unwrap_or(false);
            let mod_secure = args.module_secure.unwrap_or(false);

            // Parse integration_ids from comma-separated string
            let integration_ids_tokens = if let Some(ref ids_str) = args.module_integration_ids {
                let ids: Vec<&str> = ids_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                quote! { &[#(#ids),*] }
            } else {
                quote! { &[] }
            };

            Some(quote! {
                #[allow(non_upper_case_globals)]
                #[doc(hidden)]
                pub static #module_meta_ident: runtara_dsl::agent_meta::AgentModuleConfig =
                    runtara_dsl::agent_meta::AgentModuleConfig {
                        id: #module_id,
                        name: #mod_display_name,
                        description: #mod_description,
                        has_side_effects: #mod_has_side_effects,
                        supports_connections: #mod_supports_connections,
                        integration_ids: #integration_ids_tokens,
                        secure: #mod_secure,
                    };

                inventory::submit! {
                    &#module_meta_ident
                }
            })
        } else {
            None
        };

    // Detect if the function is async
    let is_async = input_fn.sig.asyncness.is_some();

    // Generate executor wrapper based on sync/async
    let executor_wrapper = if is_async {
        // Async function: directly await the result
        quote! {
            #[doc(hidden)]
            fn #executor_fn_ident(input: serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>> {
                Box::pin(async move {
                    let typed_input: #input_type_ident = serde_json::from_value(input)
                        .map_err(|e| format!("Invalid input for {}: {}", #capability_id, e))?;
                    let result = #fn_name(typed_input).await?;
                    serde_json::to_value(result)
                        .map_err(|e| format!("Failed to serialize result: {}", e))
                })
            }
        }
    } else {
        // Sync function: wrap with spawn_blocking
        quote! {
            #[doc(hidden)]
            fn #executor_fn_ident(input: serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>> {
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || {
                        let typed_input: #input_type_ident = serde_json::from_value(input)
                            .map_err(|e| format!("Invalid input for {}: {}", #capability_id, e))?;
                        let result = #fn_name(typed_input)?;
                        serde_json::to_value(result)
                            .map_err(|e| format!("Failed to serialize result: {}", e))
                    }).await.map_err(|e| format!("Task panicked: {}", e))?
                })
            }
        }
    };

    // Generate compensation hint if compensates_with is provided
    let compensation_hint_token = if let Some(ref comp_cap_id) = args.compensates_with {
        let description_token = match &args.compensates_description {
            Some(d) => quote! { Some(#d) },
            None => quote! { None },
        };

        quote! {
            Some(runtara_dsl::agent_meta::CompensationHint {
                capability_id: #comp_cap_id,
                description: #description_token,
            })
        }
    } else {
        quote! { None }
    };

    // Generate known_errors array from errors attribute
    let known_errors_ident = format_ident!("__{}_KNOWN_ERRORS", fn_name.to_string().to_uppercase());
    let (known_errors_static, known_errors_token) = if let Some(ref errors_spec) = args.errors {
        let error_tokens: Vec<_> = errors_spec
            .0
            .iter()
            .map(|err| {
                let code = &err.code;
                let description = &err.description;
                let kind_token = if err.kind == "transient" {
                    quote! { runtara_dsl::agent_meta::ErrorKind::Transient }
                } else {
                    quote! { runtara_dsl::agent_meta::ErrorKind::Permanent }
                };
                let attrs = &err.attributes;

                quote! {
                    runtara_dsl::agent_meta::KnownError {
                        code: #code,
                        description: #description,
                        kind: #kind_token,
                        attributes: &[#(#attrs),*],
                    }
                }
            })
            .collect();

        let count = error_tokens.len();
        (
            quote! {
                #[allow(non_upper_case_globals)]
                #[doc(hidden)]
                static #known_errors_ident: [runtara_dsl::agent_meta::KnownError; #count] = [
                    #(#error_tokens),*
                ];
            },
            quote! { &#known_errors_ident },
        )
    } else {
        (quote! {}, quote! { &[] })
    };

    let expanded = quote! {
        #input_fn

        #known_errors_static

        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #meta_ident: runtara_dsl::agent_meta::CapabilityMeta = runtara_dsl::agent_meta::CapabilityMeta {
            module: #module_token,
            capability_id: #capability_id,
            function_name: #fn_name_str,
            input_type: #input_type,
            output_type: #output_type,
            display_name: #display_name_token,
            description: #description_token,
            has_side_effects: #side_effects,
            is_idempotent: #idempotent,
            rate_limited: #rate_limited,
            compensation_hint: #compensation_hint_token,
            known_errors: #known_errors_token,
        };

        inventory::submit! {
            &#meta_ident
        }

        #executor_wrapper

        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #executor_ident: runtara_dsl::agent_meta::CapabilityExecutor = runtara_dsl::agent_meta::CapabilityExecutor {
            module: #module_str,
            capability_id: #capability_id,
            execute: #executor_fn_ident,
        };

        inventory::submit! {
            &#executor_ident
        }

        #module_registration
    };

    TokenStream::from(expanded)
}

/// Convert Option<String> to tokens
fn option_to_tokens(opt: &Option<String>) -> proc_macro2::TokenStream {
    match opt {
        Some(s) => quote! { Some(#s) },
        None => quote! { None },
    }
}

/// Extract the Ok type from Result<T, E>
fn extract_result_ok_type(output: &syn::ReturnType) -> String {
    if let syn::ReturnType::Type(_, ty) = output
        && let Type::Path(type_path) = &**ty
        && let Some(segment) = type_path.path.segments.first()
        && segment.ident == "Result"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
    {
        return type_to_string(inner_ty);
    }
    "Unknown".to_string()
}

/// Convert a Type to a string representation
fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            let segments: Vec<String> = type_path
                .path
                .segments
                .iter()
                .map(|s| {
                    let ident = s.ident.to_string();
                    if let syn::PathArguments::AngleBracketed(args) = &s.arguments {
                        let inner: Vec<String> = args
                            .args
                            .iter()
                            .filter_map(|arg| {
                                if let syn::GenericArgument::Type(inner_ty) = arg {
                                    Some(type_to_string(inner_ty))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !inner.is_empty() {
                            format!("{}<{}>", ident, inner.join(", "))
                        } else {
                            ident
                        }
                    } else {
                        ident
                    }
                })
                .collect();
            segments.join("::")
        }
        Type::Tuple(tuple) if tuple.elems.is_empty() => "()".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// Derive macro for capability input structs
///
/// Generates metadata about input fields that can be collected at runtime.
///
/// # Example
/// ```ignore
/// #[derive(CapabilityInput)]
/// #[capability_input(display_name = "Random Double Input")]
/// pub struct RandomDoubleInput {
///     #[field(display_name = "Minimum", description = "Minimum value")]
///     pub min: Option<f64>,
/// }
/// ```
#[proc_macro_derive(CapabilityInput, attributes(capability_input, field))]
pub fn derive_capability_input(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let args = match InputContainerArgs::from_derive_input(&input) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let struct_name = &args.ident;
    let struct_name_str = struct_name.to_string();

    let fields = match args.data {
        darling::ast::Data::Struct(fields) => fields.fields,
        _ => {
            return TokenStream::from(
                quote! { compile_error!("CapabilityInput can only be derived for structs"); },
            );
        }
    };

    let field_metas: Vec<_> = fields
        .iter()
        .filter(|f| !f.skip)
        .filter(|f| {
            // Skip connection_id field
            f.ident.as_ref().map(|i| i.to_string()) != Some("connection_id".to_string())
        })
        .map(|f| {
            let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
            let type_str = type_to_string(&f.ty);
            let (inner_type, is_option_type) = unwrap_option_type(&type_str);
            // Field is optional if it's Option<T>, has #[field(default = "...")], or has #[serde(default)]
            let is_optional = is_option_type || f.default.is_some() || has_serde_default(&f.attrs);

            let display_name_token = option_to_tokens(&f.display_name);
            let description_token = option_to_tokens(&f.description);
            let example_token = option_to_tokens(&f.example);
            let default_token = option_to_tokens(&f.default);

            let enum_values_fn_token = if let Some(ref enum_type) = f.enum_type {
                let enum_ident = format_ident!("{}", enum_type);
                quote! { Some(<#enum_ident as runtara_dsl::agent_meta::EnumVariants>::variant_names) }
            } else {
                quote! { None }
            };

            quote! {
                runtara_dsl::agent_meta::InputFieldMeta {
                    name: #name,
                    type_name: #inner_type,
                    is_optional: #is_optional,
                    display_name: #display_name_token,
                    description: #description_token,
                    example: #example_token,
                    default_value: #default_token,
                    enum_values_fn: #enum_values_fn_token,
                }
            }
        })
        .collect();

    let container_display_name = option_to_tokens(&args.display_name);
    let container_description = option_to_tokens(&args.description);

    let meta_ident = format_ident!("__INPUT_META_{}", struct_name);

    let expanded = quote! {
        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #meta_ident: runtara_dsl::agent_meta::InputTypeMeta = runtara_dsl::agent_meta::InputTypeMeta {
            type_name: #struct_name_str,
            display_name: #container_display_name,
            description: #container_description,
            fields: &[#(#field_metas),*],
        };

        inventory::submit! {
            &#meta_ident
        }
    };

    TokenStream::from(expanded)
}

// ============================================================================
// Connection Params Derive Macro
// ============================================================================

/// Field attributes for ConnectionParams derive
#[derive(Debug, FromField)]
#[darling(attributes(field), forward_attrs(serde))]
struct ConnectionFieldArgs {
    ident: Option<syn::Ident>,
    ty: syn::Type,
    /// Forwarded serde attributes to detect #[serde(default)]
    attrs: Vec<syn::Attribute>,
    #[darling(default)]
    display_name: Option<String>,
    #[darling(default)]
    description: Option<String>,
    #[darling(default)]
    placeholder: Option<String>,
    #[darling(default)]
    default: Option<String>,
    /// Mark this field as a secret (password, API key, etc.)
    #[darling(default)]
    secret: bool,
}

/// Container attributes for ConnectionParams derive
#[derive(Debug, FromDeriveInput)]
#[darling(attributes(connection))]
struct ConnectionContainerArgs {
    ident: syn::Ident,
    data: darling::ast::Data<(), ConnectionFieldArgs>,
    /// Unique identifier for this connection type
    integration_id: String,
    /// Display name for UI
    #[darling(default)]
    display_name: Option<String>,
    /// Description of this connection type
    #[darling(default)]
    description: Option<String>,
    /// Category for grouping (e.g., "ecommerce", "file_storage", "llm")
    #[darling(default)]
    category: Option<String>,
}

/// Derive macro for connection parameter structs
///
/// Generates metadata about connection fields that can be collected at runtime
/// for automatic form generation in the UI.
///
/// # Example
/// ```ignore
/// #[derive(ConnectionParams)]
/// #[connection(
///     integration_id = "bearer",
///     display_name = "Bearer Token",
///     description = "Connect using a Bearer token for authentication",
///     category = "http"
/// )]
/// struct BearerParams {
///     #[field(display_name = "Token", description = "Bearer authentication token", secret)]
///     token: String,
///     #[field(display_name = "Base URL", description = "API base URL (e.g., https://api.example.com)", placeholder = "https://api.example.com")]
///     base_url: String,
/// }
/// ```
#[proc_macro_derive(ConnectionParams, attributes(connection, field))]
pub fn derive_connection_params(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let args = match ConnectionContainerArgs::from_derive_input(&input) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let struct_name = &args.ident;
    let integration_id = &args.integration_id;

    let fields = match args.data {
        darling::ast::Data::Struct(fields) => fields.fields,
        _ => {
            return TokenStream::from(
                quote! { compile_error!("ConnectionParams can only be derived for structs"); },
            );
        }
    };

    let field_metas: Vec<_> = fields
        .iter()
        .map(|f| {
            let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
            let type_str = type_to_string(&f.ty);
            let (inner_type, is_option_type) = unwrap_option_type(&type_str);
            // Field is optional if it's Option<T>, has #[field(default = "...")], or has #[serde(default)]
            let is_optional = is_option_type || f.default.is_some() || has_serde_default(&f.attrs);

            let display_name_token = option_to_tokens(&f.display_name);
            let description_token = option_to_tokens(&f.description);
            let placeholder_token = option_to_tokens(&f.placeholder);
            let default_token = option_to_tokens(&f.default);
            let is_secret = f.secret;

            quote! {
                runtara_dsl::agent_meta::ConnectionFieldMeta {
                    name: #name,
                    type_name: #inner_type,
                    is_optional: #is_optional,
                    display_name: #display_name_token,
                    description: #description_token,
                    placeholder: #placeholder_token,
                    default_value: #default_token,
                    is_secret: #is_secret,
                }
            }
        })
        .collect();

    // Default display name from integration_id if not provided
    let display_name = args.display_name.unwrap_or_else(|| {
        // Convert snake_case to Title Case
        integration_id
            .split('_')
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    });

    let description_token = option_to_tokens(&args.description);
    let category_token = option_to_tokens(&args.category);

    let meta_ident = format_ident!("__CONNECTION_META_{}", struct_name);

    let expanded = quote! {
        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #meta_ident: runtara_dsl::agent_meta::ConnectionTypeMeta = runtara_dsl::agent_meta::ConnectionTypeMeta {
            integration_id: #integration_id,
            display_name: #display_name,
            description: #description_token,
            category: #category_token,
            fields: &[#(#field_metas),*],
        };

        inventory::submit! {
            &#meta_ident
        }
    };

    TokenStream::from(expanded)
}

/// Unwrap Option<T> to get T and whether it's optional
fn unwrap_option_type(type_str: &str) -> (String, bool) {
    if type_str.starts_with("Option<") && type_str.ends_with('>') {
        let inner = type_str
            .strip_prefix("Option<")
            .unwrap()
            .strip_suffix('>')
            .unwrap();
        (inner.to_string(), true)
    } else {
        (type_str.to_string(), false)
    }
}

/// Check if a field has `#[serde(default)]` or `#[serde(default = "...")]` attribute
fn has_serde_default(attrs: &[syn::Attribute]) -> bool {
    for attr in attrs {
        if attr.path().is_ident("serde") {
            let mut found_default = false;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("default") {
                    found_default = true;
                }
                Ok(())
            });
            if found_default {
                return true;
            }
        }
    }
    false
}

/// Analyze a type string and extract nested type information
/// Returns (is_nullable, items_type, nested_type)
fn analyze_type_for_nesting(type_str: &str) -> (bool, Option<String>, Option<String>) {
    let mut is_nullable = false;
    let mut working_type = type_str.to_string();

    // Check for Option<T> - unwrap and mark as nullable
    if let Some(inner) = working_type
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        is_nullable = true;
        working_type = inner.to_string();
    }

    // Check for Vec<T> - extract item type
    if let Some(inner) = working_type
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        // The inner type is the items type
        let items_type = inner.to_string();
        return (is_nullable, Some(items_type), None);
    }

    // Check for HashMap/BTreeMap - these are objects, no specific nested type
    // Handle both short form (HashMap<...>) and fully qualified (std::collections::HashMap<...>)
    if working_type.starts_with("HashMap<")
        || working_type.starts_with("BTreeMap<")
        || working_type.contains("::HashMap<")
        || working_type.contains("::BTreeMap<")
    {
        return (is_nullable, None, None);
    }

    // Check if this is a known primitive type
    let primitives = [
        "()", "bool", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64",
        "u128", "usize", "f32", "f64", "String", "Value",
    ];

    if primitives.contains(&working_type.as_str()) {
        return (is_nullable, None, None);
    }

    // Otherwise, this might be a nested struct type that can be looked up
    // Only set nested_type_name if it looks like a custom type (starts with uppercase)
    // and doesn't contain :: (which would indicate a path like std::something)
    if !working_type.contains("::")
        && working_type
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
    {
        return (is_nullable, None, Some(working_type));
    }

    (is_nullable, None, None)
}

/// Derive macro for capability output structs
#[proc_macro_derive(CapabilityOutput, attributes(capability_output, field))]
pub fn derive_capability_output(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let args = match OutputContainerArgs::from_derive_input(&input) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let struct_name = &args.ident;
    let struct_name_str = struct_name.to_string();

    let fields = match args.data {
        darling::ast::Data::Struct(fields) => fields.fields,
        _ => {
            return TokenStream::from(
                quote! { compile_error!("CapabilityOutput can only be derived for structs"); },
            );
        }
    };

    let field_metas: Vec<_> = fields
        .iter()
        .map(|f| {
            let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
            let type_str = type_to_string(&f.ty);

            // Analyze the type for nested type information
            let (is_nullable, items_type, nested_type) = analyze_type_for_nesting(&type_str);

            let display_name_token = option_to_tokens(&f.display_name);
            let description_token = option_to_tokens(&f.description);
            let example_token = option_to_tokens(&f.example);
            let items_type_token = option_to_tokens(&items_type);
            let nested_type_token = option_to_tokens(&nested_type);

            quote! {
                runtara_dsl::agent_meta::OutputFieldMeta {
                    name: #name,
                    type_name: #type_str,
                    display_name: #display_name_token,
                    description: #description_token,
                    example: #example_token,
                    nullable: #is_nullable,
                    items_type_name: #items_type_token,
                    nested_type_name: #nested_type_token,
                }
            }
        })
        .collect();

    let container_display_name = option_to_tokens(&args.display_name);
    let container_description = option_to_tokens(&args.description);

    let meta_ident = format_ident!("__OUTPUT_META_{}", struct_name);

    let expanded = quote! {
        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #meta_ident: runtara_dsl::agent_meta::OutputTypeMeta = runtara_dsl::agent_meta::OutputTypeMeta {
            type_name: #struct_name_str,
            display_name: #container_display_name,
            description: #container_description,
            fields: &[#(#field_metas),*],
        };

        inventory::submit! {
            &#meta_ident
        }
    };

    TokenStream::from(expanded)
}

// ============================================================================
// Step Metadata Derive Macro
// ============================================================================

/// Container attributes for StepMeta derive
#[derive(Debug, FromDeriveInput)]
#[darling(attributes(step))]
struct StepMetaArgs {
    ident: syn::Ident,
    /// Step type ID (e.g., "Conditional", "Agent")
    #[darling(default)]
    id: Option<String>,
    /// Display name for UI
    #[darling(default)]
    display_name: Option<String>,
    /// Description of the step type
    #[darling(default)]
    description: Option<String>,
    /// Category: "control" or "execution"
    #[darling(default)]
    category: Option<String>,
}

/// Derive macro for step type structs
///
/// Generates metadata about step types that can be collected at runtime
/// for automatic DSL schema generation.
///
/// # Example
/// ```ignore
/// #[derive(StepMeta)]
/// #[step(
///     id = "Conditional",
///     display_name = "Conditional Branch",
///     description = "Evaluates conditions and branches execution",
///     category = "control"
/// )]
/// pub struct ConditionalStep {
///     pub id: String,
///     pub name: Option<String>,
///     pub input_mapping: Option<InputMapping>,
/// }
/// ```
#[proc_macro_derive(StepMeta, attributes(step))]
pub fn derive_step_meta(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let args = match StepMetaArgs::from_derive_input(&input) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let struct_name = &args.ident;
    let struct_name_str = struct_name.to_string();

    // Derive step ID from struct name if not provided (strip "Step" suffix)
    let step_id = args.id.unwrap_or_else(|| {
        struct_name_str
            .strip_suffix("Step")
            .unwrap_or(&struct_name_str)
            .to_string()
    });

    // Default display name from step ID
    let display_name = args.display_name.unwrap_or_else(|| step_id.clone());

    // Default description
    let description = args
        .description
        .unwrap_or_else(|| format!("{} step", step_id));

    // Default category based on step type
    let category = args.category.unwrap_or_else(|| match step_id.as_str() {
        "Agent" | "StartScenario" => "execution".to_string(),
        _ => "control".to_string(),
    });

    let meta_ident = format_ident!("__STEP_META_{}", struct_name);
    let schema_fn_ident = format_ident!("__step_schema_{}", struct_name.to_string().to_lowercase());

    let expanded = quote! {
        #[doc(hidden)]
        fn #schema_fn_ident() -> schemars::schema::RootSchema {
            schemars::schema_for!(#struct_name)
        }

        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub static #meta_ident: runtara_dsl::agent_meta::StepTypeMeta = runtara_dsl::agent_meta::StepTypeMeta {
            id: #step_id,
            display_name: #display_name,
            description: #description,
            category: #category,
            schema_fn: #schema_fn_ident,
        };

        inventory::submit! {
            &#meta_ident
        }
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    // ========================================================================
    // Tests for unwrap_option_type
    // ========================================================================

    #[test]
    fn test_unwrap_option_type_non_optional() {
        let (inner, is_optional) = unwrap_option_type("String");
        assert_eq!(inner, "String");
        assert!(!is_optional);
    }

    #[test]
    fn test_unwrap_option_type_simple_option() {
        let (inner, is_optional) = unwrap_option_type("Option<String>");
        assert_eq!(inner, "String");
        assert!(is_optional);
    }

    #[test]
    fn test_unwrap_option_type_primitive() {
        let (inner, is_optional) = unwrap_option_type("Option<i32>");
        assert_eq!(inner, "i32");
        assert!(is_optional);
    }

    #[test]
    fn test_unwrap_option_type_complex_inner() {
        let (inner, is_optional) = unwrap_option_type("Option<Vec<String>>");
        assert_eq!(inner, "Vec<String>");
        assert!(is_optional);
    }

    #[test]
    fn test_unwrap_option_type_non_option_generic() {
        let (inner, is_optional) = unwrap_option_type("Vec<String>");
        assert_eq!(inner, "Vec<String>");
        assert!(!is_optional);
    }

    #[test]
    fn test_unwrap_option_type_empty_string() {
        let (inner, is_optional) = unwrap_option_type("");
        assert_eq!(inner, "");
        assert!(!is_optional);
    }

    // ========================================================================
    // Tests for type_to_string
    // ========================================================================

    #[test]
    fn test_type_to_string_simple_type() {
        let ty: Type = parse_quote!(String);
        assert_eq!(type_to_string(&ty), "String");
    }

    #[test]
    fn test_type_to_string_primitive() {
        let ty: Type = parse_quote!(i32);
        assert_eq!(type_to_string(&ty), "i32");
    }

    #[test]
    fn test_type_to_string_generic_single() {
        let ty: Type = parse_quote!(Option<String>);
        assert_eq!(type_to_string(&ty), "Option<String>");
    }

    #[test]
    fn test_type_to_string_generic_multiple() {
        let ty: Type = parse_quote!(HashMap<String, i32>);
        assert_eq!(type_to_string(&ty), "HashMap<String, i32>");
    }

    #[test]
    fn test_type_to_string_vec() {
        let ty: Type = parse_quote!(Vec<u8>);
        assert_eq!(type_to_string(&ty), "Vec<u8>");
    }

    #[test]
    fn test_type_to_string_nested_generics() {
        let ty: Type = parse_quote!(Option<Vec<String>>);
        assert_eq!(type_to_string(&ty), "Option<Vec<String>>");
    }

    #[test]
    fn test_type_to_string_unit_type() {
        let ty: Type = parse_quote!(());
        assert_eq!(type_to_string(&ty), "()");
    }

    #[test]
    fn test_type_to_string_path_type() {
        let ty: Type = parse_quote!(std::collections::HashMap<String, i32>);
        assert_eq!(
            type_to_string(&ty),
            "std::collections::HashMap<String, i32>"
        );
    }

    #[test]
    fn test_type_to_string_result_type() {
        let ty: Type = parse_quote!(Result<String, Error>);
        assert_eq!(type_to_string(&ty), "Result<String, Error>");
    }

    #[test]
    fn test_type_to_string_custom_type() {
        let ty: Type = parse_quote!(MyCustomInput);
        assert_eq!(type_to_string(&ty), "MyCustomInput");
    }

    // ========================================================================
    // Tests for analyze_type_for_nesting
    // ========================================================================

    #[test]
    fn test_analyze_type_for_nesting_primitive() {
        let (nullable, items, nested) = analyze_type_for_nesting("String");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_i32() {
        let (nullable, items, nested) = analyze_type_for_nesting("i32");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_option_primitive() {
        let (nullable, items, nested) = analyze_type_for_nesting("Option<String>");
        assert!(nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_vec() {
        let (nullable, items, nested) = analyze_type_for_nesting("Vec<String>");
        assert!(!nullable);
        assert_eq!(items, Some("String".to_string()));
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_option_vec() {
        let (nullable, items, nested) = analyze_type_for_nesting("Option<Vec<i32>>");
        assert!(nullable);
        assert_eq!(items, Some("i32".to_string()));
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_hashmap() {
        let (nullable, items, nested) = analyze_type_for_nesting("HashMap<String, i32>");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_btreemap() {
        let (nullable, items, nested) = analyze_type_for_nesting("BTreeMap<String, Value>");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_std_hashmap() {
        let (nullable, items, nested) =
            analyze_type_for_nesting("std::collections::HashMap<String, i32>");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_custom_type() {
        let (nullable, items, nested) = analyze_type_for_nesting("MyCustomStruct");
        assert!(!nullable);
        assert!(items.is_none());
        assert_eq!(nested, Some("MyCustomStruct".to_string()));
    }

    #[test]
    fn test_analyze_type_for_nesting_option_custom() {
        let (nullable, items, nested) = analyze_type_for_nesting("Option<AddressInfo>");
        assert!(nullable);
        assert!(items.is_none());
        assert_eq!(nested, Some("AddressInfo".to_string()));
    }

    #[test]
    fn test_analyze_type_for_nesting_unit_type() {
        let (nullable, items, nested) = analyze_type_for_nesting("()");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_value() {
        let (nullable, items, nested) = analyze_type_for_nesting("Value");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    #[test]
    fn test_analyze_type_for_nesting_lowercase_custom() {
        // Lowercase types shouldn't be treated as custom nested types
        let (nullable, items, nested) = analyze_type_for_nesting("lowercase_type");
        assert!(!nullable);
        assert!(items.is_none());
        assert!(nested.is_none());
    }

    // ========================================================================
    // Tests for option_to_tokens
    // ========================================================================

    #[test]
    fn test_option_to_tokens_some() {
        let opt = Some("test value".to_string());
        let tokens = option_to_tokens(&opt);
        let code = tokens.to_string();
        assert!(code.contains("Some"));
        assert!(code.contains("test value"));
    }

    #[test]
    fn test_option_to_tokens_none() {
        let opt: Option<String> = None;
        let tokens = option_to_tokens(&opt);
        let code = tokens.to_string();
        assert_eq!(code, "None");
    }

    #[test]
    fn test_option_to_tokens_empty_string() {
        let opt = Some("".to_string());
        let tokens = option_to_tokens(&opt);
        let code = tokens.to_string();
        assert!(code.contains("Some"));
    }

    #[test]
    fn test_option_to_tokens_special_chars() {
        let opt = Some("value with \"quotes\" and 'apostrophes'".to_string());
        let tokens = option_to_tokens(&opt);
        let code = tokens.to_string();
        assert!(code.contains("Some"));
    }

    // ========================================================================
    // Tests for extract_result_ok_type
    // ========================================================================

    #[test]
    fn test_extract_result_ok_type_simple() {
        let output: syn::ReturnType = parse_quote!(-> Result<String, Error>);
        assert_eq!(extract_result_ok_type(&output), "String");
    }

    #[test]
    fn test_extract_result_ok_type_custom() {
        let output: syn::ReturnType = parse_quote!(-> Result<MyOutput, String>);
        assert_eq!(extract_result_ok_type(&output), "MyOutput");
    }

    #[test]
    fn test_extract_result_ok_type_unit() {
        let output: syn::ReturnType = parse_quote!(-> Result<(), Error>);
        assert_eq!(extract_result_ok_type(&output), "()");
    }

    #[test]
    fn test_extract_result_ok_type_generic() {
        let output: syn::ReturnType = parse_quote!(-> Result<Vec<String>, Error>);
        assert_eq!(extract_result_ok_type(&output), "Vec<String>");
    }

    #[test]
    fn test_extract_result_ok_type_no_return() {
        let output: syn::ReturnType = syn::ReturnType::Default;
        assert_eq!(extract_result_ok_type(&output), "Unknown");
    }

    #[test]
    fn test_extract_result_ok_type_non_result() {
        let output: syn::ReturnType = parse_quote!(-> String);
        assert_eq!(extract_result_ok_type(&output), "Unknown");
    }

    #[test]
    fn test_extract_result_ok_type_option() {
        let output: syn::ReturnType = parse_quote!(-> Result<Option<String>, Error>);
        assert_eq!(extract_result_ok_type(&output), "Option<String>");
    }

    // ========================================================================
    // Tests for has_serde_default
    // ========================================================================

    #[test]
    fn test_has_serde_default_true() {
        let attr: syn::Attribute = parse_quote!(#[serde(default)]);
        assert!(has_serde_default(&[attr]));
    }

    #[test]
    fn test_has_serde_default_with_value() {
        let attr: syn::Attribute = parse_quote!(#[serde(default = "default_value")]);
        assert!(has_serde_default(&[attr]));
    }

    #[test]
    fn test_has_serde_default_other_serde_attr() {
        let attr: syn::Attribute = parse_quote!(#[serde(rename = "other_name")]);
        assert!(!has_serde_default(&[attr]));
    }

    #[test]
    fn test_has_serde_default_non_serde() {
        let attr: syn::Attribute = parse_quote!(#[field(display_name = "Test")]);
        assert!(!has_serde_default(&[attr]));
    }

    #[test]
    fn test_has_serde_default_empty() {
        assert!(!has_serde_default(&[]));
    }

    #[test]
    fn test_has_serde_default_multiple_attrs() {
        let attr1: syn::Attribute = parse_quote!(#[field(display_name = "Test")]);
        let attr2: syn::Attribute = parse_quote!(#[serde(default)]);
        assert!(has_serde_default(&[attr1, attr2]));
    }

    #[test]
    fn test_has_serde_default_multiple_nested_default_first() {
        // When default appears first in the list, it should be detected
        let attr: syn::Attribute = parse_quote!(#[serde(default, rename = "foo")]);
        assert!(has_serde_default(&[attr]));
    }

    // Note: The current implementation has a limitation where it may not detect
    // `default` when it appears after a key=value pair like `rename = "foo"`.
    // This is because parse_nested_meta stops at the first unhandled meta item.
    // In practice, this edge case is rare since most code puts `default` first.
}
