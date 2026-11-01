use crate::component::{convert_from_snake_case, drain_filter, is_option, unwrap_option, Docs};
use proc_macro2::{Ident, TokenStream};
use quote::{quote, ToTokens};
use rust_syntax::{parse::Parse, parse_quote, Field, ItemStruct, LitStr, Meta, Type, Visibility};

pub struct Model {
    docs: Docs,
    vis: Visibility,
    name: Ident,
    props: Vec<Prop>,
    body: ItemStruct,
}

impl Parse for Model {
    fn parse(input: rust_syntax::parse::ParseStream) -> rust_syntax::Result<Self> {
        let mut item = ItemStruct::parse(input)?;

        let docs = Docs::new(&item.attrs)?;

        let props = item
            .fields
            .clone()
            .into_iter()
            .map(Prop::new)
            .collect::<rust_syntax::Result<Vec<_>>>()?;

        // We need to remove the `#[doc = ""]` and `#[builder(_)]`
        // attrs from the function signature
        drain_filter(&mut item.attrs, |attr| match &attr.meta {
            Meta::NameValue(attr) => attr.path == parse_quote!(doc),
            Meta::List(attr) => attr.path == parse_quote!(prop),
            _ => false,
        });
        item.fields.iter_mut().for_each(|arg| {
            drain_filter(&mut arg.attrs, |attr| match &attr.meta {
                Meta::NameValue(attr) => attr.path == parse_quote!(doc),
                Meta::List(attr) => attr.path == parse_quote!(prop),
                _ => false,
            });
        });

        Ok(Self {
            docs,
            vis: item.vis.clone(),
            name: convert_from_snake_case(&item.ident),
            props,
            body: item,
        })
    }
}

impl Model {
    pub(crate) fn expand(&self) -> rust_syntax::Result<TokenStream> {
        let Self {
            docs,
            vis,
            name,
            props,
            body,
        } = self;

        let declaration_generics = &body.generics;
        let (impl_generics, generics, where_clause) = body.generics.split_for_impl();

        let prop_builder_fields = prop_builder_fields(vis, props)?;
        let prop_docs = generate_prop_docs(props)?;
        let builder_name_doc = LitStr::new(&format!("Props for the [`{name}`] slot."), name.span());

        let output = quote! {
            #[doc = #builder_name_doc]
            #[doc = ""]
            #docs
            #prop_docs
            #[derive(::web_interface::typed_builder::TypedBuilder)]
            #[builder(doc, crate_module_path=::web_interface::typed_builder)]
            #vis struct #name #declaration_generics #where_clause {
                #prop_builder_fields
            }

            impl #impl_generics From<#name #generics> for Vec<#name #generics> #where_clause {
                fn from(value: #name #generics) -> Self {
                    vec![value]
                }
            }

            /*impl #impl_generics ::web_interface::Props for #name #generics #where_clause {
                type Builder = #builder_name #generics;
                fn builder() -> Self::Builder {
                    #name::builder()
                }
            }

            impl #impl_generics ::web_interface::DynAttrs for #name #generics #where_clause {
                fn dyn_attrs(mut self, v: Vec<(&'static str, ::web_interface::Attribute)>) -> Self {
                    #dyn_attrs_props
                    self
                }
            }*/
        };

        Ok(output)
    }
}

struct Prop {
    docs: Docs,
    prop_opts: PropOpt,
    name: Ident,
    ty: Type,
}

impl Prop {
    fn new(arg: Field) -> rust_syntax::Result<Self> {
        let prop_opts = PropOpt::from_attributes(&arg.attrs)?;

        let Some(name) = arg.ident.clone() else {
            return Err(rust_syntax::Error::new_spanned(
                &arg,
                "only `prop: bool` style types are allowed within the \
                 `#[slot]` macro",
            ));
        };

        Ok(Self {
            docs: Docs::new(&arg.attrs)?,
            prop_opts,
            name,
            ty: arg.ty,
        })
    }
}

type PropOpt = crate::prop_options::PropOptions<false>;

struct TypedBuilderOpts<'a> {
    default: bool,
    default_with_value: Option<rust_syntax::Expr>,
    strip_option: bool,
    into: bool,
    ty: &'a Type,
}

impl<'a> TypedBuilderOpts<'a> {
    pub fn from_opts(opts: &PropOpt, ty: &'a Type) -> Self {
        Self {
            default: opts.is_optional(),
            default_with_value: opts.default.clone(),
            strip_option: opts.strip_option || opts.optional && is_option(ty),
            into: opts.into,
            ty,
        }
    }
}

impl TypedBuilderOpts<'_> {
    fn expand(&self) -> rust_syntax::Result<TokenStream> {
        let default = if let Some(v) = &self.default_with_value {
            let v = v.to_token_stream().to_string();
            quote! { default_code=#v, }
        } else if self.default {
            quote! { default, }
        } else {
            quote! {}
        };

        // If self.strip_option && self.into, then the strip_option will be represented as part of the transform closure.
        let strip_option = if self.strip_option && !self.into {
            quote! { strip_option, }
        } else {
            quote! {}
        };

        let into = if self.into {
            if !self.strip_option {
                let ty = &self.ty;
                quote! {
                    fn transform<__IntoReactiveValueMarker>(value: impl ::web_interface::prelude::IntoReactiveValue<#ty, __IntoReactiveValueMarker>) -> #ty {
                        value.into_reactive_value()
                    },
                }
            } else {
                let ty = unwrap_option(self.ty)?;
                quote! {
                    fn transform<__IntoReactiveValueMarker>(value: impl ::web_interface::prelude::IntoReactiveValue<#ty, __IntoReactiveValueMarker>) -> Option<#ty> {
                        Some(value.into_reactive_value())
                    },
                }
            }
        } else {
            quote! {}
        };

        let setter = if !strip_option.is_empty() || !into.is_empty() {
            quote! { setter(#strip_option #into) }
        } else {
            quote! {}
        };

        let output = if !default.is_empty() || !setter.is_empty() {
            quote! { #[builder(#default #setter)] }
        } else {
            quote! {}
        };

        Ok(output)
    }
}

fn prop_builder_fields(vis: &Visibility, props: &[Prop]) -> rust_syntax::Result<TokenStream> {
    props
        .iter()
        .map(|prop| {
            let Prop {
                docs,
                name,
                prop_opts,
                ty,
            } = prop;

            let builder_attrs = TypedBuilderOpts::from_opts(prop_opts, ty).expand()?;

            let builder_docs = prop_to_doc(prop, PropDocStyle::Inline)?;

            Ok(quote! {
                #docs
                #builder_docs
                #builder_attrs
                #vis #name: #ty,
            })
        })
        .collect()
}

fn generate_prop_docs(props: &[Prop]) -> rust_syntax::Result<TokenStream> {
    let required_prop_docs = props
        .iter()
        .filter(|Prop { prop_opts, .. }| !prop_opts.is_optional())
        .map(|p| prop_to_doc(p, PropDocStyle::List))
        .collect::<rust_syntax::Result<TokenStream>>()?;

    let optional_prop_docs = props
        .iter()
        .filter(|Prop { prop_opts, .. }| prop_opts.is_optional())
        .map(|p| prop_to_doc(p, PropDocStyle::List))
        .collect::<rust_syntax::Result<TokenStream>>()?;

    let required_prop_docs = if !required_prop_docs.is_empty() {
        quote! {
            #[doc = "# Required Props"]
            #required_prop_docs
        }
    } else {
        quote! {}
    };

    let optional_prop_docs = if !optional_prop_docs.is_empty() {
        quote! {
            #[doc = "# Optional Props"]
            #optional_prop_docs
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #required_prop_docs
        #optional_prop_docs
    })
}

#[derive(Clone, Copy)]
enum PropDocStyle {
    List,
    Inline,
}

fn prop_to_doc(
    Prop {
        docs,
        name,
        ty,
        prop_opts,
    }: &Prop,
    style: PropDocStyle,
) -> rust_syntax::Result<TokenStream> {
    let ty = if (prop_opts.optional || prop_opts.strip_option) && is_option(ty) {
        unwrap_option(ty)?
    } else {
        ty.to_owned()
    };

    let type_item: rust_syntax::Item = parse_quote! {
        type SomeType = #ty;
    };

    let file = rust_syntax::File {
        shebang: None,
        frontmatter: None,
        attrs: vec![],
        items: vec![type_item],
    };

    let pretty_ty = rust_source_format::unparse(&file);

    let pretty_ty = &pretty_ty[16..&pretty_ty.len() - 2];

    match style {
        PropDocStyle::List => {
            let arg_ty_doc = LitStr::new(
                &if !prop_opts.into {
                    format!("- **{}**: [`{}`]", quote!(#name), pretty_ty)
                } else {
                    format!("- **{}**: `impl`[`Into<{}>`]", quote!(#name), pretty_ty)
                },
                name.span(),
            );

            let arg_user_docs = docs.padded();

            Ok(quote! {
                #[doc = #arg_ty_doc]
                #arg_user_docs
            })
        }
        PropDocStyle::Inline => {
            let arg_ty_doc = LitStr::new(
                &if !prop_opts.into {
                    format!(
                        "**{}**: [`{}`]{}",
                        quote!(#name),
                        pretty_ty,
                        docs.typed_builder()
                    )
                } else {
                    format!(
                        "**{}**: `impl`[`Into<{}>`]{}",
                        quote!(#name),
                        pretty_ty,
                        docs.typed_builder()
                    )
                },
                name.span(),
            );

            Ok(quote! {
                #[builder(setter(doc = #arg_ty_doc))]
            })
        }
    }
}

#[cfg(test)]
#[path = "../../../test/macros/unit/slot/prop_doc_tests.rs"]
mod prop_doc_tests;
