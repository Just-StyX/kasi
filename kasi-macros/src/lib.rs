extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprArray, ExprLit, ExprPath, Fields, FnArg, Ident, ItemFn,
    Lit::{self},
    LitStr, Token,
    parse::Parse,
    parse_macro_input, parse2,
};

#[proc_macro_attribute]
pub fn endpoint(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_stream = proc_macro2::TokenStream::from(args);
    let parsed_args = match parse_endpoint_args(args_stream) {
        Ok(res) => res,
        Err(err) => return TokenStream::from(err.to_compile_error()),
    };

    let method_ident = parsed_args.method;
    let path_lit = parsed_args.path;

    let input_fn = parse_macro_input!(input as ItemFn);

    let security_metadata = parse_security_attributes(&input_fn.attrs);
    let response_metadata = parse_response_attribute(&input_fn.attrs);

    let fn_vis = &input_fn.vis;
    let fn_ident = &input_fn.sig.ident;
    let inner_fn_ident = quote::format_ident!("__kasi_handler_{}", fn_ident);
    let fn_inputs = &input_fn.sig.inputs;
    let fn_block = &input_fn.block;
    let fn_output = &input_fn.sig.output;

    let auth_context_var = quote::format_ident!("__global_auth_context");

    // 0. EXTRACT OR INITIALIZE REQUEST CONTEXT FOR TRACING
    let context_init_block = quote! {
        let __req_ctx = __active_req
            .extensions()
            .get::<kasi_jb::prelude::RequestContext>()
            .cloned()
            .unwrap_or_else(|| kasi_jb::prelude::RequestContext::from_request(&__active_req));
    };

    // 1. GENERATE SECURITY ENFORCEMENT CODE (WITH TRACE CONTEXT)
    let security_check_block = if let Some(meta) = security_metadata {
        let required_roles = meta.roles;
        let required_scopes = meta.scopes;

        quote! {
            let #auth_context_var = match kasi_jb::prelude::AuthContext::verify(&__active_req, &[#(#required_roles),*], &[#(#required_scopes),*]).await {
                Ok(ctx) => ctx,
                Err(err) => return err.into_response_with_context(&__req_ctx),
            };
        }
    } else {
        quote! {
            let #auth_context_var = kasi_jb::prelude::AuthContext::default_anonymous();
        }
    };

    // 2. GENERATE OPENAPI RESPONSE DOCUMENTATION REGISTRATION CODE
    let mut doc_registrations = Vec::new();
    for resp in response_metadata {
        let status = resp.status;
        let description = resp.description;

        let json_value = match resp.json_model {
            Some(model_name) => quote! { Some(#model_name) },
            None => quote! { None },
        };

        doc_registrations.push(quote! {
            endpoint_meta.add_response(#status, #description, #json_value);
        });
    }

    // 3. PARAMETER & EXTRACTOR EVALUATION SEQUENCES (WITH TRACE CONTEXT & BODY DETECTION)
    let mut extraction_blocks = Vec::new();
    let mut handler_arguments = Vec::new();
    let mut tx_variable_ident = None;
    let mut detected_body_model: Option<String> = None;

    for (index, arg) in fn_inputs.iter().enumerate() {
        if let FnArg::Typed(pat_type) = arg {
            let ty = &pat_type.ty;
            let var_name = quote::format_ident!("__extracted_arg_{}", index);

            // FIXED: Detect Json<Model> or ValidatedJson<Model> while `ty` is in scope
            if detected_body_model.is_none() {
                detected_body_model = extract_json_body_model(ty);
            }

            let is_auth_context = quote!(#ty).to_string().contains("AuthContext");
            let is_transaction = quote!(#ty).to_string().contains("Tx");

            if is_transaction {
                tx_variable_ident = Some(var_name.clone());
            }

            if is_auth_context {
                extraction_blocks.push(quote! {
                    let #var_name = #auth_context_var.clone();
                });
            } else {
                extraction_blocks.push(quote! {
                    let (#var_name, __updated_req) = match <#ty as kasi_jb::prelude::FromRequest>::extract(__active_req).await {
                        Ok(tuple_res) => tuple_res,
                        Err(err) => return err.into_response_with_context(&__req_ctx),
                    };
                    __active_req = __updated_req;
                });
            }

            handler_arguments.push(quote! { #var_name });
        }
    }

    // 3b. BUILD OPENAPI REQUEST BODY REGISTRATION TOKENS
    let request_body_registration = if let Some(rb) = parse_request_body_attribute(&input_fn.attrs)
    {
        let model = rb.json_model;
        let req_flag = rb.required;
        let desc_tokens = match rb.description {
            Some(d) => quote! { Some(#d) },
            None => quote! { None },
        };
        quote! { endpoint_meta.set_request_body(#model, #desc_tokens, #req_flag); }
    } else if let Some(model) = detected_body_model {
        quote! { endpoint_meta.set_request_body(#model, None, true); }
    } else {
        quote! {}
    };

    // 4. TRANSACTION ENFORCEMENT MATCH STRATEGY
    let execution_pipeline_block = if let Some(tx_ident) = tx_variable_ident {
        quote! {
            match #inner_fn_ident(#(#handler_arguments),*).await {
                Ok(res) => {
                    if let Err(commit_err) = #tx_ident.0.commit().await {
                        kasi_jb::prelude::Error::Internal(format!("Transaction commit routine failure: {}", commit_err))
                            .into_response_with_context(&__req_ctx)
                    } else {
                        kasi_jb::prelude::IntoResponse::into_response(res)
                    }
                }
                Err(handler_err) => {
                    let _ = #tx_ident.0.rollback().await;
                    kasi_jb::prelude::IntoResponse::into_response(handler_err)
                }
            }
        }
    } else {
        quote! {
            kasi_jb::prelude::IntoResponse::into_response(#inner_fn_ident(#(#handler_arguments),*).await)
        }
    };

    // 5. EXPAND UNIT STRUCT + HIDDEN HANDLER FUNCTION
    let expanded = quote! {
        #[doc(hidden)]
        #fn_vis async fn #inner_fn_ident(#fn_inputs) #fn_output #fn_block

        #[allow(non_camel_case_types)]
        #fn_vis struct #fn_ident;

        impl kasi_jb::prelude::RegisterEndpoint for #fn_ident {
            fn register<R: kasi_jb::prelude::EndpointRegistry>(self, registry: R) -> R {
                let wrapped_handler: kasi_jb::prelude::HandlerFn = std::sync::Arc::new(|req| {
                    Box::pin(async move {
                        let (parts, incoming_body) = req.into_parts();
                        let boxed_incoming_body = http_body_util::BodyExt::map_err(incoming_body, hyper::Error::from).boxed();
                        let mut __active_req = http::Request::from_parts(parts, boxed_incoming_body);

                        #context_init_block
                        #security_check_block
                        #(#extraction_blocks)*

                        #execution_pipeline_block
                    })
                });

                let mut endpoint_meta = kasi_jb::prelude::EndpointMetadata::new();
                #(#doc_registrations)*
                #request_body_registration

                registry.mount_endpoint(
                    kasi_jb::prelude::Method::#method_ident,
                    #path_lit,
                    wrapped_handler,
                    endpoint_meta
                )
            }
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_derive(IntoResponse, attributes(problem))] // <-- Register 'problem' as a valid helper attribute
pub fn derive_into_response(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = match &input.data {
        // ========================================================
        // PATH A: ENUM HANDLING (RFC 7807 Problem Details Response)
        // ========================================================
        Data::Enum(data_enum) => {
            let mut variant_match_arms = Vec::new();

            for variant in &data_enum.variants {
                let variant_ident = &variant.ident;
                let variant_str = variant_ident.to_string();

                let mut status_code: u16 = 500;
                let mut title_str = variant_str.clone();

                for attr in &variant.attrs {
                    if attr.path().is_ident("problem") {
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("status") {
                                let expr: Expr = meta.value()?.parse()?;
                                if let Expr::Lit(ExprLit {
                                    lit: Lit::Int(i), ..
                                }) = expr
                                {
                                    status_code = i.base10_parse().unwrap_or(500);
                                }
                            } else if meta.path.is_ident("title") {
                                let expr: Expr = meta.value()?.parse()?;
                                if let Expr::Lit(ExprLit {
                                    lit: Lit::Str(s), ..
                                }) = expr
                                {
                                    title_str = s.value();
                                }
                            }
                            Ok(())
                        });
                    }
                }

                let match_arm = match &variant.fields {
                    Fields::Named(_) => {
                        quote! {
                            Self::#variant_ident { .. } => {
                                (#status_code, #title_str, format!("Application error event occurred: {}", #title_str))
                            }
                        }
                    }
                    Fields::Unnamed(_) => {
                        quote! {
                            Self::#variant_ident(..) => {
                                (#status_code, #title_str, format!("Application error event occurred: {}", #title_str))
                            }
                        }
                    }
                    Fields::Unit => {
                        quote! {
                            Self::#variant_ident => {
                                (#status_code, #title_str, #title_str.to_string())
                            }
                        }
                    }
                };
                variant_match_arms.push(match_arm);
            }

            quote! {
                impl #impl_generics kasi_jb::prelude::IntoResponse for #name #ty_generics #where_clause {
                    fn into_response(self) -> kasi_jb::prelude::Response<kasi_jb::prelude::BoxBody> {
                        let (status_code, title, detail) = match self {
                            #(#variant_match_arms),*
                        };

                        let json_payload = serde_json::json!({
                            "type": format!("https://kasi.local{}", title.to_lowercase().replace(" ", "-")),
                            "title": title,
                            "status": status_code,
                            "detail": detail
                        });

                        let body_bytes = bytes::Bytes::from(json_payload.to_string());
                        let boxed_body = http_body_util::Full::new(body_bytes)
                            .map_err(|e| match e {})
                            .boxed();

                        kasi_jb::prelude::Response::builder()
                            .status(status_code)
                            .header("Content-Type", "application/problem+json")
                            .body(boxed_body)
                            .unwrap()
                    }
                }
            }
        }

        // ========================================================
        // PATH B: STRUCT HANDLING (Standard JSON Entity Response)
        // ========================================================
        Data::Struct(_) => {
            quote! {
                impl #impl_generics kasi_jb::prelude::IntoResponse for #name #ty_generics #where_clause {
                    fn into_response(self) -> kasi_jb::prelude::Response<kasi_jb::prelude::BoxBody> {
                        let json_string = match serde_json::to_string(&self) {
                            Ok(s) => s,
                            Err(e) => format!(r#"{{"error": "Serialization failure: {}"}}"#, e),
                        };

                        let body = http_body_util::Full::new(bytes::Bytes::from(json_string))
                            .map_err(|e| match e {})
                            .boxed();

                        kasi_jb::prelude::Response::builder()
                            .status(kasi_jb::prelude::StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(body)
                            .unwrap()
                    }
                }
            }
        }

        Data::Union(_) => {
            syn::Error::new_spanned(name, "IntoResponse custom derive cannot be used on Unions")
                .to_compile_error()
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn main(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(input as ItemFn);

    let fn_vis = &input_fn.vis;
    let fn_ident = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_output = &input_fn.sig.output;
    let fn_attrs = &input_fn.attrs;

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &input_fn.sig.ident,
            "Kasi main entrypoint function must be declared as 'async fn main()'",
        )
        .into_compile_error()
        .into();
    }

    if !input_fn.sig.inputs.is_empty() {
        return syn::Error::new_spanned(
            &input_fn.sig.inputs,
            "Kasi main function cannot take any input arguments.",
        )
        .into_compile_error()
        .into();
    }

    let expanded = quote! {
        #(#fn_attrs)*
        #fn_vis fn #fn_ident() #fn_output {
            async fn __inner_async_main() #fn_output #fn_block

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to initialize the core framework Tokio async runtime environment.");

            runtime.block_on(__inner_async_main())
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let mut checks = Vec::new();
    let mut regex_statics = Vec::new();

    if let Data::Struct(data_struct) = &input.data {
        if let Fields::Named(fields) = &data_struct.fields {
            for field in &fields.named {
                let field_ident = &field.ident;
                let field_str = field_ident.as_ref().unwrap().to_string();

                for attr in &field.attrs {
                    if attr.path().is_ident("validate") {
                        let _ = attr.parse_nested_meta(|meta| {
                            // 1. Email check
                            if meta.path.is_ident("email") {
                                checks.push(quote! {
                                    if !self.#field_ident.contains('@') {
                                        return Err(Error::BadRequest(format!(
                                            "Validation failure on field '{}': Invalid email address format.", #field_str
                                        )));
                                    }
                                });
                            }
                            // 2. Length check
                            else if meta.path.is_ident("min_len") {
                                let expr: syn::Expr = meta.value()?.parse()?;
                                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) = expr {
                                    let min: usize = i.base10_parse().unwrap_or(0);
                                    checks.push(quote! {
                                        if self.#field_ident.len() < #min {
                                            return Err(Error::BadRequest(format!(
                                                "Validation failure on field '{}': Length must be at least {} characters.", #field_str, #min
                                            )));
                                        }
                                    });
                                }
                            }
                            // 3. Numeric range check (e.g. range(min = 18, max = 99))
                            else if meta.path.is_ident("range") {
                                let mut min_val: Option<syn::Expr> = None;
                                let mut max_val: Option<syn::Expr> = None;
                                meta.parse_nested_meta(|inner| {
                                    if inner.path.is_ident("min") {
                                        min_val = Some(inner.value()?.parse()?);
                                    } else if inner.path.is_ident("max") {
                                        max_val = Some(inner.value()?.parse()?);
                                    }
                                    Ok(())
                                })?;

                                if let Some(min) = min_val {
                                    checks.push(quote! {
                                        if (self.#field_ident as f64) < (#min as f64) {
                                            return Err(Error::BadRequest(format!(
                                                "Validation failure on field '{}': Value cannot be less than {}.", #field_str, #min
                                            )));
                                        }
                                    });
                                }
                                if let Some(max) = max_val {
                                    checks.push(quote! {
                                        if (self.#field_ident as f64) > (#max as f64) {
                                            return Err(Error::BadRequest(format!(
                                                "Validation failure on field '{}': Value cannot be greater than {}.", #field_str, #max
                                            )));
                                        }
                                    });
                                }
                            }
                            // 4. Regex validation rules (e.g. regex = "^[a-zA-Z0-9]+$")
                            else if meta.path.is_ident("regex") {
                                let expr: syn::Expr = meta.value()?.parse()?;
                                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = expr {
                                    let pattern = s.value();
                                    let static_regex_ident = quote::format_ident!("__REGEX_{}", field_str.to_uppercase());
                                    // Generate a lazy compiled global regex instance to avoid runtime parsing overhead
                                    regex_statics.push(quote! {
                                        static #static_regex_ident: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                                    });

                                    checks.push(quote! {
                                        let re = #static_regex_ident.get_or_init(|| {
                                            regex::Regex::new(#pattern).expect("Invalid macro-compiled regex pattern declaration spec.")
                                        });
                                        if !re.is_match(&self.#field_ident.to_string()) {
                                            return Err(Error::BadRequest(format!(
                                                "Validation failure on field '{}': String format does not match required layout constraints.", #field_str
                                            )));
                                        }
                                    });
                                }
                            }
                            Ok(())
                        });
                    }
                }
            }
        }
    }

    let expanded = quote! {
        #(#regex_statics)*

        impl #impl_generics Validate for #name #ty_generics #where_clause {
            fn validate(&self) -> Result<(), Error> {
                #(#checks)*
                Ok(())
            }
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_derive(Schema)]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let name_str = name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let mut property_insertions = Vec::new();
    let mut required_fields = Vec::new();

    if let Data::Struct(data_struct) = &input.data
        && let Fields::Named(fields) = &data_struct.fields
    {
        for field in &fields.named {
            let field_ident = field.ident.as_ref().unwrap();
            let field_name = field_ident.to_string();
            let (schema_tokens, is_optional) = map_rust_type_to_openapi(&field.ty);

            property_insertions.push(quote! {
                properties.insert(#field_name.to_string(), #schema_tokens);
            });

            if !is_optional {
                required_fields.push(quote! {
                    required.push(#field_name.to_string());
                });
            }
        }
    }

    let expanded = quote! {
        impl #impl_generics kasi_jb::prelude::Schema for #name #ty_generics #where_clause {
            fn schema_name() -> &'static str {
                #name_str
            }

            fn schema_object() -> kasi_jb::prelude::SchemaObject {
                let mut properties = std::collections::HashMap::new();
                let mut required = Vec::new();

                #(#property_insertions)*
                #(#required_fields)*

                kasi_jb::prelude::SchemaObject {
                    schema_type: Some("object".to_string()),
                    properties: Some(properties),
                    required: if required.is_empty() { None } else { Some(required) },
                    ..Default::default()
                }
            }
        }
    };

    TokenStream::from(expanded)
}

/// Helper that translates Rust AST types into OpenAPI 3.1.0 SchemaObject tokens.
fn map_rust_type_to_openapi(ty: &syn::Type) -> (proc_macro2::TokenStream, bool) {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        let ident_str = segment.ident.to_string();

        // Handle Option<T> -> unwrap inner type and mark field as not required
        if ident_str == "Option"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
        {
            let (inner_schema, _) = map_rust_type_to_openapi(inner_ty);
            return (inner_schema, true);
        }

        // Handle Vec<T> -> array schema with inner items
        if ident_str == "Vec"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
        {
            let (inner_schema, _) = map_rust_type_to_openapi(inner_ty);
            return (
                quote! {
                    kasi_jb::prelude::SchemaObject {
                        schema_type: Some("array".to_string()),
                        items: Some(Box::new(#inner_schema)),
                        ..Default::default()
                    }
                },
                false,
            );
        }

        // Handle primitive types and standard library types
        let primitive_match = match ident_str.as_str() {
            "String" | "str" => Some(("string", None)),
            "Uuid" => Some(("string", Some("uuid"))),
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                Some(("integer", None))
            }
            "f32" | "f64" => Some(("number", None)),
            "bool" => Some(("boolean", None)),
            _ => None,
        };

        if let Some((openapi_type, format_opt)) = primitive_match {
            let format_tokens = match format_opt {
                Some(fmt) => quote! { Some(#fmt.to_string()) },
                None => quote! { None },
            };
            return (
                quote! {
                    kasi_jb::prelude::SchemaObject {
                        schema_type: Some(#openapi_type.to_string()),
                        format: #format_tokens,
                        ..Default::default()
                    }
                },
                false,
            );
        }

        // Fallback: assume custom struct implementing `Schema` and link via `$ref`
        let ref_path = format!("#/components/schemas/{}", ident_str);
        return (
            quote! {
                kasi_jb::prelude::SchemaObject {
                    reference: Some(#ref_path.to_string()),
                    ..Default::default()
                }
            },
            false,
        );
    }

    // Default fallback for complex or unrecognized types
    (
        quote! {
            kasi_jb::prelude::SchemaObject {
                schema_type: Some("object".to_string()),
                ..Default::default()
            }
        },
        false,
    )
}

// ====================
// Helpers
// ====================
struct EndpointArgs {
    method: Ident,
    path: LitStr,
}

struct RequestBodyAttr {
    json_model: String,
    description: Option<String>,
    required: bool,
}

fn parse_endpoint_args(tokens: proc_macro2::TokenStream) -> syn::Result<EndpointArgs> {
    struct ArgsParser {
        method: Ident,
        _comma: Token![,],
        path: LitStr,
    }

    impl Parse for ArgsParser {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            Ok(ArgsParser {
                method: input.parse()?,
                _comma: input.parse()?,
                path: input.parse()?,
            })
        }
    }
    let parsed = parse2::<ArgsParser>(tokens)?;
    Ok(EndpointArgs {
        method: parsed.method,
        path: parsed.path,
    })
}

#[derive(Debug, Clone)]
struct SecurityAttr {
    roles: Vec<String>,
    scopes: Vec<String>,
}

#[derive(Debug, Clone)]
struct ResponseAttr {
    status: u16,
    description: String,
    json_model: Option<String>,
}

fn parse_security_attributes(attrs: &[Attribute]) -> Option<SecurityAttr> {
    for attr in attrs {
        if attr.path().is_ident("security") {
            let mut roles = Vec::new();
            let mut scopes = Vec::new();

            let parse_result = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("RequireRole") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = expr
                    {
                        roles.push(s.value());
                    }
                } else if meta.path.is_ident("OAuth2") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Array(ExprArray { elems, .. }) = expr {
                        for elem in elems {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Str(s), ..
                            }) = elem
                            {
                                scopes.push(s.value());
                            }
                        }
                    }
                }
                Ok(())
            });

            if parse_result.is_ok() {
                return Some(SecurityAttr { roles, scopes });
            }
        }
    }
    None
}

fn parse_response_attribute(attrs: &[Attribute]) -> Vec<ResponseAttr> {
    let mut responses = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("response") {
            let mut status: u16 = 200;
            let mut description = String::new();
            let mut json_model = None;

            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("status") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Int(i), ..
                    }) = expr
                    {
                        status = i.base10_parse().unwrap_or(200);
                    }
                } else if meta.path.is_ident("description") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = expr
                    {
                        description = s.value();
                    }
                } else if meta.path.is_ident("json") {
                    let expr: Expr = meta.value()?.parse()?;
                    // Option A: Handles unquoted identifiers -> #[response(json = UserProfile)]
                    // FIXED: Changed TypePath to ExprPath matching syn's AST layout
                    if let Expr::Path(ExprPath { path, .. }) = &expr {
                        if let Some(ident) = path.get_ident() {
                            json_model = Some(ident.to_string());
                        }
                    }
                    // Option B: Handles string literals -> #[response(json = "UserProfile")]
                    else if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = expr
                    {
                        json_model = Some(s.value());
                    }
                }
                Ok(())
            });
            responses.push(ResponseAttr {
                status,
                description,
                json_model,
            });
        }
    }
    responses
}

fn parse_request_body_attribute(attrs: &[Attribute]) -> Option<RequestBodyAttr> {
    for attr in attrs {
        if attr.path().is_ident("request_body") {
            let mut json_model = None;
            let mut description = None;
            let mut required = true;

            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("json") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Path(ExprPath { path, .. }) = &expr
                        && let Some(ident) = path.get_ident()
                    {
                        json_model = Some(ident.to_string());
                    } else if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = expr
                    {
                        json_model = Some(s.value());
                    }
                } else if meta.path.is_ident("description") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = expr
                    {
                        description = Some(s.value());
                    }
                } else if meta.path.is_ident("required") {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Bool(b), ..
                    }) = expr
                    {
                        required = b.value();
                    }
                }
                Ok(())
            });

            if let Some(model) = json_model {
                return Some(RequestBodyAttr {
                    json_model: model,
                    description,
                    required,
                });
            }
        }
    }
    None
}

/// Extracts inner type name from `Json<MyModel>` or `ValidatedJson<MyModel>`.
fn extract_json_body_model(ty: &syn::Type) -> Option<String> {
    if let syn::Type::Path(type_path) = ty
        && let Some(seg) = type_path.path.segments.last()
    {
        let outer = seg.ident.to_string();
        if (outer == "Json" || outer == "ValidatedJson")
            && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
            && let Some(syn::GenericArgument::Type(syn::Type::Path(inner_path))) = args.args.first()
            && let Some(inner_seg) = inner_path.path.segments.last()
        {
            return Some(inner_seg.ident.to_string());
        }
    }
    None
}
