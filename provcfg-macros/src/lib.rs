use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Derives the companion `*Partial`/`*Prov` types and the impls that make a
/// plain config struct provenance-tracking.
///
/// Generated items for a struct `Foo`:
///
/// - `FooPartial`: every leaf wrapped in `Option` (`Deserialize` + `Serialize`).
/// - `FooProv`: every leaf wrapped in `ValueHistory` (`Serialize`).
/// - `impl Provenance for FooProv`: the defaults layer plus per-source merge.
/// - `impl From<&Foo> for FooPartial` and `impl From<&FooProv> for Foo`.
///
/// The base struct must derive `Default + Clone + serde::Deserialize`. Mark
/// sub-struct fields with `#[configurable(nested)]` to recurse; see
/// `provcfg::Configurable` for the full attribute reference.
#[proc_macro_derive(Configurable, attributes(configurable))]
pub fn configurable_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let all_fields = match extract_named_fields(&ast) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };
    // Fields marked `#[configurable(skip)]` are not part of the config schema.
    // They stay on the user struct but are invisible to everything we generate.
    // The user struct still has them, so `From<&Prov>` fills them via `Default`.
    let has_skipped = all_fields.iter().any(|f| f.skip);
    let fields: Vec<Field> = all_fields.into_iter().filter(|f| !f.skip).collect();

    let partial_struct = generate_partial(&ast.ident, &fields);
    let partial_from_ref = generate_partial_from_ref(&ast.ident, &fields);
    let prov_struct = generate_prov(&ast.ident, &fields);
    let provenance_impl = generate_provenance_impl(&ast.ident, &fields);
    let prov_serialize = generate_prov_serialize(&ast.ident, &fields);
    let prov_into_user = generate_prov_into_user(&ast.ident, &fields, has_skipped);

    let expanded = quote! {
        #partial_struct
        #partial_from_ref
        #prov_struct
        #provenance_impl
        #prov_serialize
        #prov_into_user
    };

    TokenStream::from(expanded)
}

struct Field {
    name: syn::Ident,
    ty: syn::Type,
    nested: bool,
    secret: bool,
    /// `true` when the user marked this field with `#[configurable(env_list)]`.
    /// Triggers a custom `deserialize_with` that accepts either an array or a
    /// comma-separated string. Required for `Vec<String>` fields populated
    /// from environment variables.
    env_list: bool,
    /// `true` when the user marked this field with `#[configurable(skip)]`.
    /// The field stays on the user struct but is omitted from the partial,
    /// prov, merge, collect_sources, and Serialize impls. Useful for fields
    /// that live alongside config (e.g. runtime state, derived caches).
    skip: bool,
    /// Optional rename applied to the partial field's serde key. Preserved
    /// verbatim; the macro emits a lowercase `#[serde(alias = ...)]` alongside
    /// so the env source (which lowercases segments) still matches.
    rename: Option<String>,
}

fn extract_named_fields(ast: &DeriveInput) -> syn::Result<Vec<Field>> {
    let named = match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &ast.ident,
                    "Configurable only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &ast.ident,
                "Configurable only supports structs",
            ));
        }
    };

    named
        .iter()
        .map(|f| {
            let flags = parse_field_attrs(&f.attrs)?;
            Ok(Field {
                name: f.ident.clone().unwrap(),
                ty: f.ty.clone(),
                nested: flags.nested,
                secret: flags.secret,
                env_list: flags.env_list,
                skip: flags.skip,
                rename: flags.rename,
            })
        })
        .collect()
}

#[derive(Default)]
struct FieldFlags {
    nested: bool,
    secret: bool,
    env_list: bool,
    skip: bool,
    rename: Option<String>,
}

fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldFlags> {
    let mut flags = FieldFlags::default();
    for attr in attrs {
        if !attr.path().is_ident("configurable") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("nested") {
                flags.nested = true;
                Ok(())
            } else if meta.path.is_ident("secret") {
                flags.secret = true;
                Ok(())
            } else if meta.path.is_ident("env_list") {
                flags.env_list = true;
                Ok(())
            } else if meta.path.is_ident("skip") {
                flags.skip = true;
                Ok(())
            } else if meta.path.is_ident("rename") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                flags.rename = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error(
                    "unknown #[configurable(...)] key; expected one of nested, secret, env_list, skip, rename",
                ))
            }
        })?;
    }
    Ok(flags)
}

fn field_key(f: &Field) -> String {
    f.rename.clone().unwrap_or_else(|| f.name.to_string())
}

fn dotted_key_join(key: &str) -> proc_macro2::TokenStream {
    quote! {
        if prefix.is_empty() {
            #key.to_string()
        } else {
            format!("{}.{}", prefix, #key)
        }
    }
}

fn rename_last_segment(ty: &syn::Type, suffix: &str) -> syn::Type {
    let mut tp = match ty {
        syn::Type::Path(tp) => tp.clone(),
        _ => panic!("#[configurable(nested)] requires a path type, e.g. `Database`"),
    };
    let last = tp
        .path
        .segments
        .last_mut()
        .expect("path type has at least one segment");
    last.ident = format_ident!("{}{}", last.ident, suffix);
    syn::Type::Path(tp)
}

fn generate_partial(base_name: &syn::Ident, fields: &[Field]) -> proc_macro2::TokenStream {
    let partial_name = format_ident!("{}Partial", base_name);

    let field_definitions = fields.iter().map(|f| {
        let name = &f.name;
        // Preserve the rename verbatim for file formats; add a lowercase alias
        // so the env source (which lowercases segments) still matches.
        let rename = f.rename.as_ref().map(|r| {
            let lc = r.to_lowercase();
            if lc == *r {
                quote! { #[serde(rename = #r)] }
            } else {
                quote! { #[serde(rename = #r, alias = #lc)] }
            }
        });
        let env_list_attr = if f.env_list {
            quote! { #[serde(deserialize_with = "provcfg::deserialize_env_list")] }
        } else {
            quote! {}
        };
        if f.nested {
            let nested_partial = rename_last_segment(&f.ty, "Partial");
            quote! { #rename #env_list_attr pub #name: Option<#nested_partial> }
        } else {
            let ty = &f.ty;
            quote! { #rename #env_list_attr pub #name: Option<#ty> }
        }
    });

    quote! {
        // `Serialize` is generated so partial-producing sources (e.g. `CliSource`)
        // can round-trip the partial through serde. `None` fields serialize to
        // `null`, which deserializes back to `None` via the same Option type.
        #[derive(serde::Deserialize, serde::Serialize, Default)]
        #[serde(default)]
        pub struct #partial_name {
            #(#field_definitions),*
        }
    }
}

fn generate_partial_from_ref(base_name: &syn::Ident, fields: &[Field]) -> proc_macro2::TokenStream {
    let partial_name = format_ident!("{}Partial", base_name);

    let field_inits = fields.iter().map(|f| {
        let name = &f.name;
        if f.nested {
            quote! { #name: Some((&value.#name).into()) }
        } else {
            quote! { #name: Some(value.#name.clone()) }
        }
    });

    quote! {
        impl ::core::convert::From<&#base_name> for #partial_name {
            fn from(value: &#base_name) -> Self {
                Self { #(#field_inits),* }
            }
        }
    }
}

fn generate_prov(base_name: &syn::Ident, fields: &[Field]) -> proc_macro2::TokenStream {
    let prov_name = format_ident!("{}Prov", base_name);

    let field_definitions = fields.iter().map(|f| {
        let name = &f.name;
        if f.nested {
            let nested_prov = rename_last_segment(&f.ty, "Prov");
            quote! { pub #name: #nested_prov }
        } else {
            let ty = &f.ty;
            quote! { pub #name: provcfg::ValueHistory<#ty> }
        }
    });

    quote! {
        pub struct #prov_name {
            #(#field_definitions),*
        }
    }
}

fn generate_prov_serialize(base_name: &syn::Ident, fields: &[Field]) -> proc_macro2::TokenStream {
    let prov_name = format_ident!("{}Prov", base_name);
    let struct_name_lit = base_name.to_string();
    let field_count = fields.len();

    let field_writes = fields.iter().map(|f| {
        let name = &f.name;
        let key = field_key(f);
        if f.nested {
            quote! { state.serialize_field(#key, &self.#name)?; }
        } else {
            quote! { state.serialize_field(#key, self.#name.value())?; }
        }
    });

    quote! {
        impl serde::Serialize for #prov_name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct as _;
                let mut state = serializer.serialize_struct(#struct_name_lit, #field_count)?;
                #(#field_writes)*
                state.end()
            }
        }
    }
}

fn generate_prov_into_user(
    base_name: &syn::Ident,
    fields: &[Field],
    has_skipped: bool,
) -> proc_macro2::TokenStream {
    let prov_name = format_ident!("{}Prov", base_name);

    let field_inits = fields.iter().map(|f| {
        let name = &f.name;
        if f.nested {
            quote! { #name: ::core::convert::From::from(&prov.#name) }
        } else {
            quote! { #name: ::core::clone::Clone::clone(prov.#name.value()) }
        }
    });

    // Skipped fields aren't tracked, so fill them from `Foo::default()`. The
    // `Configurable` derive already requires `Foo: Default`. Trailing-comma
    // form keeps `Self { ..Default::default() }` valid when every field is
    // skipped.
    let body = if has_skipped {
        quote! {
            Self {
                #(#field_inits,)*
                ..::core::default::Default::default()
            }
        }
    } else {
        quote! {
            Self {
                #(#field_inits),*
            }
        }
    };

    quote! {
        impl ::core::convert::From<&#prov_name> for #base_name {
            fn from(prov: &#prov_name) -> Self {
                #body
            }
        }
    }
}

fn generate_provenance_impl(base_name: &syn::Ident, fields: &[Field]) -> proc_macro2::TokenStream {
    let prov_name = format_ident!("{}Prov", base_name);
    let partial_name = format_ident!("{}Partial", base_name);

    let leaf_history_inits = fields.iter().filter(|f| !f.nested).map(|f| {
        let name = &f.name;
        if f.secret {
            quote! { let mut #name = provcfg::ValueHistory::new().mark_secret(); }
        } else {
            quote! { let mut #name = provcfg::ValueHistory::new(); }
        }
    });

    let nested_partial_inits = fields.iter().filter(|f| f.nested).map(|f| {
        let name = &f.name;
        let nested_partial = rename_last_segment(&f.ty, "Partial");
        quote! {
            let mut #name: ::std::vec::Vec<Option<#nested_partial>> =
                ::std::vec::Vec::with_capacity(partials.len());
        }
    });

    let leaf_default_pushes = fields.iter().filter(|f| !f.nested).map(|f| {
        let name = &f.name;
        quote! {
            #name.push(provcfg::Value {
                value: defaults_partial.#name.expect("defaults partial must populate every leaf field"),
                source: ::core::clone::Clone::clone(&defaults_src),
            });
        }
    });

    let per_source_steps = fields.iter().map(|f| {
        let name = &f.name;
        if f.nested {
            quote! {
                match partial {
                    Some(ref mut p) => #name.push(::core::mem::take(&mut p.#name)),
                    None => #name.push(None),
                }
            }
        } else {
            quote! {
                if let Some(ref mut p) = partial
                    && let Some(v) = ::core::mem::take(&mut p.#name)
                {
                    #name.push(provcfg::Value {
                        value: v,
                        source: ::core::clone::Clone::clone(source),
                    });
                }
            }
        }
    });

    let nested_merge_calls = fields.iter().filter(|f| f.nested).map(|f| {
        let name = &f.name;
        let nested_prov = rename_last_segment(&f.ty, "Prov");
        quote! {
            let #name = <#nested_prov as provcfg::Provenance>::merge(sources, #name);
        }
    });

    let field_names = fields.iter().map(|f| &f.name);

    let collect_steps = fields.iter().map(|f| {
        let name = &f.name;
        let key = field_key(f);
        let joined = dotted_key_join(&key);
        if f.nested {
            quote! {
                {
                    let next_prefix = #joined;
                    self.#name.collect_sources(&next_prefix, out);
                }
            }
        } else {
            quote! {
                {
                    let key = #joined;
                    out.insert(key, self.#name.source().category());
                }
            }
        }
    });

    let walk_steps = fields.iter().map(|f| {
        let name = &f.name;
        let key = field_key(f);
        let joined = dotted_key_join(&key);
        if f.nested {
            quote! {
                {
                    let next_prefix = #joined;
                    self.#name.walk_leaves(&next_prefix, visitor);
                }
            }
        } else {
            quote! {
                {
                    let key = #joined;
                    visitor(
                        &key,
                        self.#name.value(),
                        self.#name.source().category(),
                        self.#name.is_secret(),
                    );
                }
            }
        }
    });

    quote! {
        impl provcfg::Provenance for #prov_name {
            type Partial = #partial_name;

            fn defaults_partial() -> Self::Partial {
                (&<#base_name>::default()).into()
            }

            fn merge(
                sources: &[provcfg::SourceArc],
                partials: Vec<Option<Self::Partial>>,
            ) -> Self {
                #(#leaf_history_inits)*
                #(#nested_partial_inits)*

                // Leaf-field defaults layer. Nested fields handle defaults via
                // their own recursive merge.
                let defaults_partial = <Self as provcfg::Provenance>::defaults_partial();
                let defaults_src: provcfg::SourceArc = provcfg::defaults_source();
                #(#leaf_default_pushes)*

                for (source, mut partial) in sources.iter().zip(partials) {
                    #(#per_source_steps)*
                }

                #(#nested_merge_calls)*

                #prov_name { #(#field_names),* }
            }

            fn collect_sources(
                &self,
                prefix: &str,
                out: &mut ::std::collections::HashMap<::std::string::String, provcfg::Category>,
            ) {
                #(#collect_steps)*
            }

            fn walk_leaves(
                &self,
                prefix: &str,
                visitor: &mut dyn ::core::ops::FnMut(
                    &str,
                    &dyn provcfg::erased_serde::Serialize,
                    provcfg::Category,
                    bool,
                ),
            ) {
                #(#walk_steps)*
            }
        }
    }
}
