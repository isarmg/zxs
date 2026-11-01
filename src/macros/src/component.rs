use crate::diagnostics::{message_with_help, Errors};
use crate::view_source::value_to_string;
use convert_case::{
    Case::{Pascal, Snake},
    Casing,
};
use iterator_toolkit::Itertools;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, quote_spanned, ToTokens, TokenStreamExt};
use rust_syntax::{
    parse::Parse, parse_quote, spanned::Spanned, token::Colon, visit_mut::VisitMut,
    AngleBracketedGenericArguments, Attribute, FnArg, GenericArgument, GenericParam, Item, ItemFn,
    LitStr, Meta, Pat, PatIdent, Path, PathArguments, ReturnType, Signature, Stmt, Type,
    TypeImplTrait, TypeParam, TypePath, Visibility,
};

pub struct Model {
    validation_error: Option<rust_syntax::Error>,
    is_transparent: bool,
    docs: Docs,
    unknown_attrs: UnknownAttrs,
    vis: Visibility,
    name: Ident,
    props: Vec<Prop>,
    body: ItemFn,
    ret: ReturnType,
}

impl Parse for Model {
    fn parse(input: rust_syntax::parse::ParseStream) -> rust_syntax::Result<Self> {
        let mut item = ItemFn::parse(input)?;

        convert_impl_trait_to_generic(&mut item.sig);

        let (docs, mut validation_error) = match Docs::new(&item.attrs) {
            Ok(docs) => (docs, None),
            Err(error) => (Docs(Vec::new()), Some(error)),
        };
        let unknown_attrs = UnknownAttrs::new(&item.attrs);

        let props = if validation_error.is_none() {
            match item
                .sig
                .inputs
                .clone()
                .into_iter()
                .map(Prop::new)
                .collect::<rust_syntax::Result<Vec<_>>>()
            {
                Ok(props) => props,
                Err(error) => {
                    validation_error = Some(error);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        // We need to remove the `#[doc = ""]` and `#[builder(_)]`
        // attrs from the function signature
        drain_filter(&mut item.attrs, |attr| match &attr.meta {
            Meta::NameValue(attr) => attr.path == parse_quote!(doc),
            Meta::List(attr) => attr.path == parse_quote!(prop),
            _ => false,
        });
        item.sig.inputs.iter_mut().for_each(|arg| {
            if let FnArg::Typed(ty) = arg {
                drain_filter(&mut ty.attrs, |attr| match &attr.meta {
                    Meta::NameValue(attr) => attr.path == parse_quote!(doc),
                    Meta::List(attr) => attr.path == parse_quote!(prop),
                    _ => false,
                });
            }
        });

        Ok(Self {
            validation_error,
            is_transparent: false,
            docs,
            unknown_attrs,
            vis: item.vis.clone(),
            name: convert_from_snake_case(&item.sig.ident),
            props,
            ret: item.sig.output.clone(),
            body: item,
        })
    }
}

// implemented manually because Vec::drain_filter is nightly only
// follows std recommended parallel
pub fn drain_filter<T>(vec: &mut Vec<T>, mut some_predicate: impl FnMut(&mut T) -> bool) {
    let mut i = 0;
    while i < vec.len() {
        if some_predicate(&mut vec[i]) {
            _ = vec.remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn convert_from_snake_case(name: &Ident) -> Ident {
    let name_str = name.to_string();
    if name_str != name_str.to_case(Snake) {
        name.clone()
    } else {
        Ident::new(&name_str.to_case(Pascal), name.span())
    }
}

impl Model {
    pub(crate) fn expand(mut self, errors: &mut Errors) -> rust_syntax::Result<TokenStream> {
        if let Some(error) = self.validation_error.take() {
            return Err(error);
        }

        let Self {
            validation_error: _,
            is_transparent,
            docs,
            unknown_attrs,
            vis,
            name,
            props,
            body,
            ret,
        } = &self;
        let no_props = props.is_empty();

        // check for components that end ;
        if !is_transparent {
            let ends_semi = body.block.stmts.iter().last().and_then(|stmt| match stmt {
                Stmt::Item(Item::Macro(mac)) => mac.semi_token.as_ref(),
                _ => None,
            });
            if let Some(semi) = ends_semi {
                errors.push(rust_syntax::Error::new(
                    semi.span(),
                    "A component that ends with a `view!` macro followed by a \
                     semicolon will return (), an empty view. This is usually \
                     an accident, not intentional, so we prevent it. If you’d \
                     like to return (), you can do it it explicitly by \
                     returning () as the last item from the component.",
                ));
            }
        }

        let (impl_generics, generics, where_clause) = body.sig.generics.split_for_impl();

        let props_name = format_ident!("{name}Props");
        let props_builder_name = format_ident!("{name}PropsBuilder");

        let prop_builder_fields = prop_builder_fields(vis, props)?;

        let prop_names = prop_names(props);

        let builder_name_doc = LitStr::new(
            &format!(" Props for the [`{name}`] component."),
            name.span(),
        );

        let component_fn_prop_docs = generate_component_fn_prop_docs(props)?;
        let docs_and_prop_docs = if component_fn_prop_docs.is_empty() {
            // Avoid generating an empty doc line in case the component has no doc and no props.
            quote! {
                #docs
            }
        } else {
            quote! {
                #docs
                #[doc = ""]
                #component_fn_prop_docs
            }
        };

        let body_name = unmodified_fn_name_from_fn_name(&body.sig.ident);
        let body_expr = quote! { #body_name(#prop_names) };

        let component = if *is_transparent {
            body_expr
        } else {
            quote! {
                ::web_interface::reactive::graph::untrack_with_diagnostics(
                    move || {
                        #body_expr
                    }
                )
            }
        };

        let props_arg = if no_props {
            quote! {}
        } else {
            quote! {
                props: #props_name #generics
            }
        };

        let destructure_props = if no_props {
            quote! {}
        } else {
            quote! {
                let #props_name {
                    #prop_names
                } = props;
            }
        };

        let body = quote! {
            #destructure_props
            #component
        };

        let output = quote! {
            #[doc = #builder_name_doc]
            #[doc = ""]
            #docs_and_prop_docs
            #[derive(::web_interface::typed_builder::TypedBuilder)]
            #[builder(crate_module_path=::web_interface::typed_builder)]
            #[allow(non_snake_case)]
            #vis struct #props_name #impl_generics #where_clause {
                #prop_builder_fields
            }

            impl #impl_generics ::web_interface::component::Props for #props_name #generics #where_clause {
                type Builder = #props_builder_name #generics;

                fn builder() -> Self::Builder {
                    #props_name::builder()
                }
            }

            #unknown_attrs
            #docs_and_prop_docs
            #[allow(non_snake_case, clippy::too_many_arguments)]
            #[allow(clippy::needless_lifetimes)]
            #vis fn #name #impl_generics (
                #props_arg
            ) #ret
            #where_clause
            {
                #body
            }
        };

        Ok(output)
    }
}

impl Model {
    #[allow(clippy::wrong_self_convention)]
    pub fn is_transparent(mut self, is_transparent: bool) -> Self {
        self.is_transparent = is_transparent;

        self
    }
}

/// A model that is more lenient in case of a syntax error in the function body,
/// but does not actually implement the behavior of the real model. This is
/// used to improve IDEs and rust-analyzer's auto-completion behavior in case
/// of a syntax error.
pub struct DummyModel {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub sig: Signature,
    pub body: TokenStream,
}

impl Parse for DummyModel {
    fn parse(input: rust_syntax::parse::ParseStream) -> rust_syntax::Result<Self> {
        let mut attrs = input.call(Attribute::parse_outer)?;
        // Drop unknown attributes like #[deprecated]
        drain_filter(&mut attrs, |attr| {
            !is_lint_attr(attr) && !attr.path().is_ident("doc")
        });

        let vis: Visibility = input.parse()?;
        let sig: Signature = input.parse()?;

        // The body is left untouched, so it will not cause an error
        // even if the syntax is invalid.
        let body: TokenStream = input.parse()?;

        Ok(Self {
            attrs,
            vis,
            sig,
            body,
        })
    }
}

impl ToTokens for DummyModel {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let Self {
            attrs,
            vis,
            sig,
            body,
        } = self;

        // Strip attributes like documentation comments and #[prop]
        // from the signature, so as to not confuse the user with incorrect
        // error messages.
        let sig = {
            let mut sig = sig.clone();
            sig.inputs.iter_mut().for_each(|arg| {
                if let FnArg::Typed(ty) = arg {
                    ty.attrs.retain(|attr| match &attr.meta {
                        Meta::List(list) => list
                            .path
                            .segments
                            .first()
                            .map(|n| n.ident != "prop")
                            .unwrap_or(true),
                        Meta::NameValue(name_value) => name_value
                            .path
                            .segments
                            .first()
                            .map(|n| n.ident != "doc")
                            .unwrap_or(true),
                        _ => true,
                    });
                }
            });
            sig
        };

        let output = quote! {
            #(#attrs)*
            #vis #sig #body
        };

        tokens.append_all(output)
    }
}

struct Prop {
    docs: Docs,
    prop_opts: PropOpt,
    name: PatIdent,
    ty: Type,
}

impl Prop {
    fn new(arg: FnArg) -> rust_syntax::Result<Self> {
        let typed = if let FnArg::Typed(ty) = arg {
            ty
        } else {
            return Err(rust_syntax::Error::new_spanned(
                arg,
                "receiver not allowed in `fn`",
            ));
        };

        let prop_opts = PropOpt::from_attributes(&typed.attrs)?;

        let name = match *typed.pat {
            Pat::Ident(i) => {
                if let Some(name) = &prop_opts.name {
                    PatIdent {
                        attrs: vec![],
                        by_ref: None,
                        mutability: None,
                        ident: name.clone(),
                        subpat: None,
                    }
                } else {
                    i
                }
            }
            pat @ (Pat::Struct(_) | Pat::Tuple(_) | Pat::TupleStruct(_)) => {
                let Some(name) = prop_opts.name.as_ref() else {
                    return Err(rust_syntax::Error::new_spanned(
                        pat,
                        "destructured props must be given a name e.g. \
                         #[prop(name = \"data\")]",
                    ));
                };
                PatIdent {
                    attrs: vec![],
                    by_ref: None,
                    mutability: None,
                    ident: name.clone(),
                    subpat: None,
                }
            }
            pat => {
                return Err(rust_syntax::Error::new_spanned(
                    pat,
                    "only `prop: bool` style types are allowed within the \
                     `#[component]` macro",
                ));
            }
        };

        Ok(Self {
            docs: Docs::new(&typed.attrs)?,
            prop_opts,
            name,
            ty: *typed.ty,
        })
    }
}

#[derive(Clone)]
pub struct Docs(Vec<(String, Span)>);

impl ToTokens for Docs {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let s = self
            .0
            .iter()
            .map(|(doc, span)| quote_spanned!(*span=> #[doc = #doc]))
            .collect::<TokenStream>();

        tokens.append_all(s);
    }
}

impl Docs {
    pub fn new(attrs: &[Attribute]) -> rust_syntax::Result<Self> {
        #[derive(Debug, Copy, Clone, PartialEq, Eq)]
        enum ViewCodeFenceState {
            Outside,
            Rust,
            Rsx,
        }
        let mut quotes = "```".to_string();
        let mut quote_ws = "".to_string();
        let mut view_code_fence_state = ViewCodeFenceState::Outside;
        // todo fix docs stuff
        const RSX_START: &str = "# ::web_interface::view! {";
        const RSX_END: &str = "# };";

        // Separated out of chain to allow rustfmt to work
        let map = |(doc, span): (String, Span)| {
            doc.split('\n')
                .map(str::trim_end)
                .flat_map(|doc| {
                    let trimmed_doc = doc.trim_start();
                    let leading_ws = &doc[..doc.len() - trimmed_doc.len()];
                    let trimmed_doc = trimmed_doc.trim_end();
                    match view_code_fence_state {
                        ViewCodeFenceState::Outside
                            if trimmed_doc.starts_with("```")
                                && trimmed_doc.trim_start_matches('`').starts_with("view") =>
                        {
                            view_code_fence_state = ViewCodeFenceState::Rust;
                            let view = trimmed_doc.find('v').unwrap();
                            trimmed_doc[..view].clone_into(&mut quotes);
                            leading_ws.clone_into(&mut quote_ws);
                            let rust_options = &trimmed_doc[view + "view".len()..].trim_start();
                            vec![
                                format!("{leading_ws}{quotes}{rust_options}"),
                                leading_ws.to_string(),
                            ]
                        }
                        ViewCodeFenceState::Rust if trimmed_doc == quotes => {
                            view_code_fence_state = ViewCodeFenceState::Outside;
                            vec![leading_ws.to_string(), doc.to_owned()]
                        }
                        ViewCodeFenceState::Rust if trimmed_doc.starts_with('<') => {
                            view_code_fence_state = ViewCodeFenceState::Rsx;
                            vec![format!("{leading_ws}{RSX_START}"), doc.to_owned()]
                        }
                        ViewCodeFenceState::Rsx if trimmed_doc == quotes => {
                            view_code_fence_state = ViewCodeFenceState::Outside;
                            vec![format!("{leading_ws}{RSX_END}"), doc.to_owned()]
                        }
                        _ => vec![doc.to_string()],
                    }
                })
                .map(|l| (l, span))
                .collect_vec()
        };

        let mut attrs = attrs
            .iter()
            .filter_map(|attr| {
                let Meta::NameValue(attr) = &attr.meta else {
                    return None;
                };
                if !attr.path.is_ident("doc") {
                    return None;
                }

                Some(
                    value_to_string(&attr.value)
                        .map(|val| (val, attr.path.span()))
                        .ok_or_else(|| {
                            rust_syntax::Error::new_spanned(
                                attr,
                                "expected string literal in value of doc \
                                 comment",
                            )
                        }),
                )
            })
            .collect::<rust_syntax::Result<Vec<_>>>()?
            .into_iter()
            .flat_map(map)
            .collect_vec();

        if view_code_fence_state != ViewCodeFenceState::Outside {
            if view_code_fence_state == ViewCodeFenceState::Rust {
                attrs.push((quote_ws.clone(), Span::call_site()))
            } else {
                attrs.push((format!("{quote_ws}{RSX_END}"), Span::call_site()))
            }
            attrs.push((format!("{quote_ws}{quotes}"), Span::call_site()))
        }

        Ok(Self(attrs))
    }

    pub fn padded(&self) -> TokenStream {
        self.0
            .iter()
            .enumerate()
            .map(|(idx, (doc, span))| {
                let doc = if idx == 0 {
                    format!("    - {doc}")
                } else {
                    format!("      {doc}")
                };

                let doc = LitStr::new(&doc, *span);

                quote! { #[doc = #doc] }
            })
            .collect()
    }

    pub fn typed_builder(&self) -> String {
        let doc_str = self.0.iter().map(|s| s.0.as_str()).join("\n");

        if doc_str.chars().filter(|c| *c != '\n').count() != 0 {
            format!("\n\n{doc_str}")
        } else {
            String::new()
        }
    }
}

fn is_lint_attr(attr: &Attribute) -> bool {
    let path = &attr.path();
    path.is_ident("allow")
        || path.is_ident("warn")
        || path.is_ident("expect")
        || path.is_ident("deny")
        || path.is_ident("forbid")
}

pub struct UnknownAttrs(Vec<(TokenStream, Span)>);

impl UnknownAttrs {
    pub fn new(attrs: &[Attribute]) -> Self {
        let attrs = attrs
            .iter()
            .filter_map(|attr| {
                if attr.path().is_ident("doc") {
                    if let Meta::NameValue(_) = &attr.meta {
                        return None;
                    }
                }

                if is_lint_attr(attr) {
                    return None;
                }

                Some((attr.into_token_stream(), attr.span()))
            })
            .collect_vec();
        Self(attrs)
    }
}

impl ToTokens for UnknownAttrs {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let s = self
            .0
            .iter()
            .map(|(attr, span)| quote_spanned!(*span=> #attr))
            .collect::<TokenStream>();
        tokens.append_all(s);
    }
}

type PropOpt = crate::prop_options::PropOptions<true>;

struct TypedBuilderOpts<'a> {
    marker: bool,
    default: bool,
    default_with_value: Option<rust_syntax::Expr>,
    strip_option: bool,
    into: bool,
    ty: &'a Type,
}

impl<'a> TypedBuilderOpts<'a> {
    fn from_opts(opts: &PropOpt, ty: &'a Type) -> Self {
        Self {
            marker: opts.marker,
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
        if self.marker {
            return Ok(quote! {
                #[builder(default, setter(skip))]
            });
        }

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

            let ident = &name.ident;

            let builder_attrs = TypedBuilderOpts::from_opts(prop_opts, ty).expand()?;

            let builder_docs = prop_to_doc(prop, PropDocStyle::Inline)?;

            // Children and markers won't need documentation in many cases
            let allow_missing_docs = if prop_opts.marker || ident == "children" {
                quote!(#[allow(missing_docs)])
            } else {
                quote!()
            };
            Ok(quote! {
                #docs
                #builder_docs
                #builder_attrs
                #allow_missing_docs
                #vis #ident: #ty,
            })
        })
        .collect()
}

fn prop_names(props: &[Prop]) -> TokenStream {
    props
        .iter()
        .map(|Prop { name, .. }| {
            // fields like mutability are removed because unneeded
            // in the contexts in which this is used
            let ident = &name.ident;
            quote! { #ident, }
        })
        .collect()
}

fn generate_component_fn_prop_docs(props: &[Prop]) -> rust_syntax::Result<TokenStream> {
    let required_prop_docs = props
        .iter()
        .filter(|Prop { prop_opts, .. }| !prop_opts.is_optional())
        .filter(|prop| !prop.prop_opts.marker)
        .map(|p| prop_to_doc(p, PropDocStyle::List))
        .collect::<rust_syntax::Result<TokenStream>>()?;

    let optional_prop_docs = props
        .iter()
        .filter(|Prop { prop_opts, .. }| prop_opts.is_optional())
        .filter(|prop| !prop.prop_opts.marker)
        .map(|p| prop_to_doc(p, PropDocStyle::List))
        .collect::<rust_syntax::Result<TokenStream>>()?;

    let required_prop_docs = if !required_prop_docs.is_empty() {
        quote! {
            #[doc = " # Required Props"]
            #required_prop_docs
        }
    } else {
        quote! {}
    };

    let optional_prop_docs = if !optional_prop_docs.is_empty() {
        quote! {
            #[doc = " # Optional Props"]
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

pub fn is_option(ty: &Type) -> bool {
    if let Type::Path(TypePath {
        path: Path { segments, .. },
        ..
    }) = ty
    {
        if let [first] = &segments.iter().collect::<Vec<_>>()[..] {
            first.ident == "Option"
        } else {
            false
        }
    } else {
        false
    }
}

pub fn unwrap_option(ty: &Type) -> rust_syntax::Result<Type> {
    const STD_OPTION_MSG: &str =
        "make sure you're not shadowing the `std::option::Option` type that \
         is automatically imported from the standard prelude";

    if let Type::Path(TypePath {
        path: Path { segments, .. },
        ..
    }) = ty
    {
        if let [first] = &segments.iter().collect::<Vec<_>>()[..] {
            if first.ident == "Option" {
                if let PathArguments::AngleBracketed(AngleBracketedGenericArguments {
                    args, ..
                }) = &first.arguments
                {
                    if let [GenericArgument::Type(ty)] = &args.iter().collect::<Vec<_>>()[..] {
                        return Ok(ty.clone());
                    }
                }
            }
        }
    }

    Err(rust_syntax::Error::new_spanned(
        ty,
        message_with_help("`Option` must be `std::option::Option`", STD_OPTION_MSG),
    ))
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
                    format!(" - **{}**: [`{pretty_ty}`]", quote!(#name))
                } else {
                    format!(
                        " - **{}**: [`impl Into<{pretty_ty}>`]({pretty_ty})",
                        quote!(#name),
                    )
                },
                name.ident.span(),
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
                name.ident.span(),
            );

            Ok(quote! {
                #[builder(setter(doc = #arg_ty_doc))]
            })
        }
    }
}

pub fn unmodified_fn_name_from_fn_name(ident: &Ident) -> Ident {
    Ident::new(
        &format!("__component_{}", ident.to_string().to_case(Snake)),
        ident.span(),
    )
}

/// Converts all `impl Trait`s in a function signature to use generic params instead.
fn convert_impl_trait_to_generic(sig: &mut Signature) {
    fn new_generic_ident(i: usize, span: Span) -> Ident {
        Ident::new(&format!("__ImplTrait{i}"), span)
    }

    // First: visit all `impl Trait`s and replace them with new generic params.
    #[derive(Default)]
    struct RemoveImplTrait(Vec<TypeImplTrait>);
    impl VisitMut for RemoveImplTrait {
        fn visit_type_mut(&mut self, ty: &mut Type) {
            rust_syntax::visit_mut::visit_type_mut(self, ty);
            if matches!(ty, Type::ImplTrait(_)) {
                let ident = new_generic_ident(self.0.len(), ty.span());
                let generic_type = Type::Path(TypePath {
                    attrs: Vec::new(),
                    qself: None,
                    path: Path::from(ident),
                });
                let Type::ImplTrait(impl_trait) = std::mem::replace(ty, generic_type) else {
                    unreachable!();
                };
                self.0.push(impl_trait);
            }
        }

        // Early exits.
        fn visit_attribute_mut(&mut self, _: &mut Attribute) {}
        fn visit_pat_mut(&mut self, _: &mut Pat) {}
    }
    let mut visitor = RemoveImplTrait::default();
    for fn_arg in sig.inputs.iter_mut() {
        visitor.visit_fn_arg_mut(fn_arg);
    }
    let RemoveImplTrait(impl_traits) = visitor;

    // Second: Add the new generic params into the signature.
    for (i, impl_trait) in impl_traits.into_iter().enumerate() {
        let span = impl_trait.span();
        let ident = new_generic_ident(i, span);
        // We can simply append to the end (only lifetime params must be first).
        // Note currently default generics are not allowed in `fn`, so not a concern.
        sig.generics.params.push(GenericParam::Type(TypeParam {
            attrs: vec![],
            ident,
            colon_token: Some(Colon { spans: [span] }),
            bounds: impl_trait.bounds,
            default: None,
        }));
    }
}

#[cfg(test)]
#[path = "../../../test/macros/unit/component/case_normalization_tests.rs"]
mod case_normalization_tests;

#[cfg(test)]
#[path = "../../../test/macros/unit/component/prop_name_tests.rs"]
mod prop_name_tests;

#[cfg(test)]
#[path = "../../../test/macros/unit/component/prop_doc_tests.rs"]
mod prop_doc_tests;
