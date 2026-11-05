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

    // Parse security and response metadata configurations out of function attributes
    let security_metadata = parse_security_attributes(&input_fn.attrs);
    let response_metadata = parse_response_attribute(&input_fn.attrs);

    let fn_vis = &input_fn.vis;
    let fn_ident = &input_fn.sig.ident;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_block = &input_fn.block;
    let fn_output = &input_fn.sig.output;

    let auth_context_var = quote::format_ident!("__global_auth_context");

    // 1. GENERATE SECURITY ENFORCEMENT CODE
    let security_check_block = if let Some(meta) = security_metadata {
        let required_roles = meta.roles;
        let required_scopes = meta.scopes;

        quote! {
            let #auth_context_var = match kasi::prelude::AuthContext::verify(&req, &[#(#required_roles),*], &[#(#required_scopes),*]).await {
                Ok(ctx) => ctx,
                Err(err) => return kasi::prelude::IntoResponse::into_response(err),
            };
        }
    } else {
        quote! {
            let #auth_context_var: Option<kasi::prelude::AuthContext> = None;
        }
    };

    // 2. GENERATE OPENAPI DOCUMENTATION REGISTRATION CODE
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

    // 3. PARAMETER & EXTRACTOR EVALUATION SEQUENCES
    let mut extraction_blocks = Vec::new();
    let mut handler_arguments = Vec::new();

    for (index, arg) in fn_inputs.iter().enumerate() {
        if let FnArg::Typed(pat_type) = arg {
            let ty = &pat_type.ty;
            let var_name = quote::format_ident!("__extracted_arg_{}", index);

            // Determine if the requested signature input parameter matches AuthContext
            let is_auth_context = quote!(#ty).to_string().contains("AuthContext");

            if is_auth_context {
                // If it is AuthContext, we inject the pre-verified profile context instead of running extract
                extraction_blocks.push(quote! {
                    let #var_name = #auth_context_var.clone();
                });
            } else {
                // For all normal request components (Json, Query, Path), run standard extraction rules
                extraction_blocks.push(quote! {
                    let #var_name = match <#ty as kasi::prelude::FromRequest>::extract(req).await {
                        Ok(val) => val,
                        Err(err) => return kasi::prelude::IntoResponse::into_response(err),
                    };
                });
            }

            handler_arguments.push(quote! { #var_name });
        }
    }

    // 4. GENERATED EXPANDED BLOCK GENERATION
    let expanded = quote! {
        #fn_vis async fn #fn_ident(#fn_inputs) #fn_output #fn_block

        #[allow(non_camel_case_type)]
        #fn_vis struct #fn_ident;

        impl kasi::prelude::RegisterRoute for #fn_ident {
            fn register_route(self, router: &mut kasi::prelude::Router) {
                let wrapped_handler: kasi::prelude::HandlerFn = std::sync::Arc::new(|req| {
                    Box::pin(async move {
                        #security_check_block
                        #(#extraction_blocks)*
                        match #fn_ident(#(#handler_arguments),*).await {
                            Ok(res) => kasi::prelude::IntoResponse::into_response(res),
                            Err(err) => kasi::prelude::IntoResponse::into_response(err),
                        }
                    })
                });

                let mut endpoint_meta = kasi::prelude::EndpointMetadata::new();
                #(#doc_registrations)*

                router.add_endpoint_with_meta(kasi::prelude::Method::#method_ident, #path_lit, wrapped_handler, endpoint_meta);
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
                impl #impl_generics kasi::prelude::IntoResponse for #name #ty_generics #where_clause {
                    fn into_response(self) -> kasi::prelude::Response<kasi::prelude::BoxBody> {
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

                        kasi::prelude::Response::builder()
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
                impl #impl_generics kasi::prelude::IntoResponse for #name #ty_generics #where_clause {
                    fn into_response(self) -> kasi::prelude::Response<kasi::prelude::BoxBody> {
                        let json_string = match serde_json::to_string(&self) {
                            Ok(s) => s,
                            Err(e) => format!(r#"{{"error": "Serialization failure: {}"}}"#, e),
                        };

                        let body = http_body_util::Full::new(bytes::Bytes::from(json_string))
                            .map_err(|e| match e {})
                            .boxed();

                        kasi::prelude::Response::builder()
                            .status(kasi::prelude::StatusCode::OK)
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

// ====================
// Helpers
// ====================
struct EndpointArgs {
    method: Ident,
    path: LitStr,
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
