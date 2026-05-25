use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Fields, parse_macro_input};

/// Generates a sibling `<Name>Args` clap-compatible struct plus a
/// `From<&<Name>Args> for <Name>Partial` impl. Intended to be derived
/// alongside `provcfg::Configurable`.
///
/// See `provcfg-clap`'s crate-level docs for the full design.
#[proc_macro_derive(ClapArgs, attributes(configurable, arg))]
pub fn clap_args_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_attrs = match parse_struct_attrs(&ast.attrs) {
        Ok(sa) => sa,
        Err(e) => return e.to_compile_error().into(),
    };
    let fields: Vec<Field> = match extract_named_fields(&ast) {
        Ok(f) => f.into_iter().filter(|f| !f.flags.skip).collect(),
        Err(e) => return e.to_compile_error().into(),
    };

    let args_name = format_ident!("{}Args", ast.ident);
    let partial_name = format_ident!("{}Partial", ast.ident);

    let args_field_defs = fields.iter().map(|f| generate_args_field(&struct_attrs, f));

    let from_field_inits = fields.iter().map(generate_from_init);

    let expanded = quote! {
        #[derive(::core::fmt::Debug, ::core::default::Default, ::core::clone::Clone, ::clap::Args)]
        pub struct #args_name {
            #(#args_field_defs)*
        }

        impl ::core::convert::From<&#args_name> for #partial_name {
            fn from(args: &#args_name) -> Self {
                Self { #(#from_field_inits),* }
            }
        }
    };

    TokenStream::from(expanded)
}

#[derive(Default)]
struct StructAttrs {
    /// Value of `#[configurable(clap_prefix = "...")]` if present. Used to
    /// auto-derive `--<prefix>-<field>` long flags for leaf fields.
    clap_prefix: Option<String>,
}

fn parse_struct_attrs(attrs: &[Attribute]) -> syn::Result<StructAttrs> {
    let mut sa = StructAttrs::default();
    for attr in attrs {
        if !attr.path().is_ident("configurable") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("clap_prefix") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                sa.clap_prefix = Some(lit.value());
                Ok(())
            } else {
                Err(meta
                    .error("unknown struct-level #[configurable(...)] key; expected `clap_prefix`"))
            }
        })?;
    }
    Ok(sa)
}

struct Field {
    name: syn::Ident,
    ty: syn::Type,
    flags: FieldFlags,
    /// All `#[arg(...)]` attributes on the user's field, forwarded verbatim
    /// onto the generated Args field.
    arg_attrs: Vec<Attribute>,
}

#[derive(Default)]
struct FieldFlags {
    nested: bool,
    skip: bool,
}

fn extract_named_fields(ast: &DeriveInput) -> syn::Result<Vec<Field>> {
    let named = match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &ast.ident,
                    "ClapArgs only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &ast.ident,
                "ClapArgs only supports structs",
            ));
        }
    };

    named
        .iter()
        .map(|f| {
            let flags = parse_field_flags(&f.attrs)?;
            let arg_attrs = f
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("arg"))
                .cloned()
                .collect();
            Ok(Field {
                name: f.ident.clone().unwrap(),
                ty: f.ty.clone(),
                flags,
                arg_attrs,
            })
        })
        .collect()
}

fn parse_field_flags(attrs: &[Attribute]) -> syn::Result<FieldFlags> {
    let mut flags = FieldFlags::default();
    for attr in attrs {
        if !attr.path().is_ident("configurable") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("nested") {
                flags.nested = true;
            } else if meta.path.is_ident("skip") {
                flags.skip = true;
            } else {
                // Other configurable flags (secret, env_list, rename, ...) are
                // consumed by the Configurable derive; consume any value so the
                // parser doesn't trip on `key = "..."`.
                let _ = meta.value();
            }
            Ok(())
        })?;
    }
    Ok(flags)
}

/// Walks a field's `#[arg(...)]` attributes and reports whether any of them
/// already sets `long` (e.g. `#[arg(long)]` or `#[arg(long = "x")]`). When
/// true, we skip the auto-derived `long` to avoid a duplicate-attribute error
/// from clap.
fn user_has_long(arg_attrs: &[Attribute]) -> bool {
    let mut found = false;
    for attr in arg_attrs {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("long") {
                found = true;
            }
            // Skip values so the parser doesn't trip on `= "..."`.
            let _ = meta.value();
            Ok(())
        });
    }
    found
}

/// Walks a field's `#[arg(...)]` attributes and reports whether any of them
/// sets `skip`. Clap's `#[arg(skip)]` means "not a CLI argument"; in that
/// case we should not auto-derive `long` either.
fn user_has_skip(arg_attrs: &[Attribute]) -> bool {
    let mut found = false;
    for attr in arg_attrs {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                found = true;
            }
            let _ = meta.value();
            Ok(())
        });
    }
    found
}

/// Walks a field's `#[arg(...)]` attributes and reports whether any of them
/// already sets `id`. When true, we don't auto-derive `id` to avoid a
/// duplicate-attribute error from clap.
fn user_has_id(arg_attrs: &[Attribute]) -> bool {
    let mut found = false;
    for attr in arg_attrs {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("id") {
                found = true;
            }
            let _ = meta.value();
            Ok(())
        });
    }
    found
}

fn auto_long(struct_attrs: &StructAttrs, field: &syn::Ident) -> String {
    let field_kebab = field.to_string().replace('_', "-");
    match &struct_attrs.clap_prefix {
        Some(prefix) => format!("{prefix}-{field_kebab}"),
        None => field_kebab,
    }
}

fn nested_args_type(ty: &syn::Type) -> syn::Type {
    let mut tp = match ty {
        syn::Type::Path(tp) => tp.clone(),
        _ => panic!("#[configurable(nested)] requires a path type"),
    };
    let last = tp
        .path
        .segments
        .last_mut()
        .expect("path type has at least one segment");
    last.ident = format_ident!("{}Args", last.ident);
    syn::Type::Path(tp)
}

fn generate_args_field(struct_attrs: &StructAttrs, f: &Field) -> proc_macro2::TokenStream {
    let name = &f.name;
    let user_arg_attrs = &f.arg_attrs;

    if f.flags.nested {
        let nested = nested_args_type(&f.ty);
        return quote! {
            #(#user_arg_attrs)*
            #[command(flatten)]
            pub #name: #nested,
        };
    }

    let ty = &f.ty;
    let user_skipped = user_has_skip(user_arg_attrs);
    let user_long = user_has_long(user_arg_attrs);
    let user_id = user_has_id(user_arg_attrs);

    // `id` defaults to the field name in clap. With a `clap_prefix`, multiple
    // sections can share a leaf name (e.g. `enabled`), so auto-deriving only
    // `long` would leave clap with duplicate ids and panic at startup. We
    // attach an `id` to match the prefixed `long` whenever we auto-derive
    // either, unless the user already supplied that piece.
    let auto_attr = if user_skipped {
        quote! {}
    } else {
        let auto = auto_long(struct_attrs, name);
        match (user_long, user_id) {
            (true, true) => quote! {},
            (true, false) => quote! { #[arg(id = #auto)] },
            (false, true) => quote! { #[arg(long = #auto)] },
            (false, false) => quote! { #[arg(id = #auto, long = #auto)] },
        }
    };

    // Use the unqualified `Option` so clap's derive recognizes it as an
    // optional argument. Clap's auto-detection of `Option<T>` is path-based
    // and trips over `::core::option::Option`.
    quote! {
        #auto_attr
        #(#user_arg_attrs)*
        pub #name: Option<#ty>,
    }
}

fn generate_from_init(f: &Field) -> proc_macro2::TokenStream {
    let name = &f.name;
    if f.flags.nested {
        // Nested Partial wraps the inner Partial in `Option`. The From for the
        // inner type produces the inner Partial; we wrap it in `Some` so the
        // outer Configurable merge sees "this source contributed to the
        // subtree". Its leaves still hold the actual `Some`/`None` choices.
        quote! { #name: ::core::option::Option::Some(::core::convert::From::from(&args.#name)) }
    } else {
        quote! { #name: ::core::clone::Clone::clone(&args.#name) }
    }
}
