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
#[darling(attributes(field))]
struct InputFieldArgs {
    ident: Option<syn::Ident>,
    ty: syn::Type,
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
            if let syn::FnArg::Typed(pat_type) = arg {
                if let Type::Path(type_path) = &*pat_type.ty {
                    return type_path.path.segments.last().map(|s| s.ident.to_string());
                }
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

    let expanded = quote! {
        #input_fn

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
        };

        inventory::submit! {
            &#meta_ident
        }

        // Executor wrapper function
        #[doc(hidden)]
        fn #executor_fn_ident(input: serde_json::Value) -> Result<serde_json::Value, String> {
            // Store raw input in thread-local for connection resolution
            runtara_dsl::agent_meta::set_current_input(&input);

            let typed_input: #input_type_ident = serde_json::from_value(input)
                .map_err(|e| format!("Invalid input for {}: {}", #capability_id, e))?;
            let result = #fn_name(typed_input)?;

            // Clear thread-local after execution
            runtara_dsl::agent_meta::clear_current_input();

            serde_json::to_value(result)
                .map_err(|e| format!("Failed to serialize result: {}", e))
        }

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
    if let syn::ReturnType::Type(_, ty) = output {
        if let Type::Path(type_path) = &**ty {
            if let Some(segment) = type_path.path.segments.first() {
                if segment.ident == "Result" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                            return type_to_string(inner_ty);
                        }
                    }
                }
            }
        }
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
            // Field is optional if it's Option<T> OR has a default value
            let is_optional = is_option_type || f.default.is_some();

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
#[darling(attributes(field))]
struct ConnectionFieldArgs {
    ident: Option<syn::Ident>,
    ty: syn::Type,
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
            // Field is optional if it's Option<T> OR has a default value
            let is_optional = is_option_type || f.default.is_some();

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
