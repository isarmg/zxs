mod component_builder;
mod slot_helper;
mod utils;

use self::{
    component_builder::component_to_tokens,
    slot_helper::{get_slot, slot_to_tokens},
};
use crate::diagnostics::Errors;
use crate::view_source::{is_component_node, value_to_string};
use convert_case::{
    Case::{Snake, UpperCamel},
    Casing,
};
use proc_macro2::{Ident, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use rstml::node::{
    CustomNode, KVAttributeValue, KeyedAttribute, Node, NodeAttribute, NodeBlock, NodeElement,
    NodeName, NodeNameFragment,
};
use rust_syntax::{
    punctuated::Pair::{End, Punctuated},
    spanned::Spanned,
    Expr::{self, Tuple},
    ExprArray, ExprLit, ExprPath, ExprRange, Lit, LitStr, RangeLimits, Stmt,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagType {
    Unknown,
    Html,
    Svg,
}

pub fn render_view(
    nodes: &mut [Node],
    global_class: Option<&TokenTree>,
    view_marker: Option<String>,
    disable_inert_html: bool,
) -> rust_syntax::Result<Option<TokenStream>> {
    let mut errors = Errors::default();
    let result = render_view_inner(
        nodes,
        global_class,
        view_marker,
        disable_inert_html,
        &mut errors,
    );
    errors.finish(result)
}

fn render_view_inner(
    nodes: &mut [Node],
    global_class: Option<&TokenTree>,
    view_marker: Option<String>,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Option<TokenStream>> {
    let disable_inert_html = disable_inert_html || global_class.is_some();

    let (base, should_add_view) = match nodes.len() {
        0 => {
            let span = Span::call_site();
            (
                Some(quote_spanned! {
                    span => ()
                }),
                false,
            )
        }
        1 => (
            node_to_tokens(
                &mut nodes[0],
                TagType::Unknown,
                None,
                global_class,
                view_marker.as_deref(),
                true,
                disable_inert_html,
                errors,
            )?,
            // only add View wrapper and view marker to a regular HTML
            // element or component, not to a <{..} /> attribute list
            match &nodes[0] {
                Node::Element(node) => !is_spread_marker(node),
                _ => false,
            },
        ),
        _ => (
            fragment_to_tokens(
                nodes,
                TagType::Unknown,
                None,
                global_class,
                view_marker.as_deref(),
                disable_inert_html,
                errors,
            )?,
            true,
        ),
    };
    Ok(base.map(|view| {
        if !should_add_view {
            view
        } else {
            quote! {
                ::web_interface::prelude::View::new(
                    #view
                )
            }
        }
    }))
}

fn is_inert_element(orig_node: &Node<impl CustomNode>) -> bool {
    // do not use this if the top-level node is not an Element,
    // or if it's an element with no children and no attrs
    match orig_node {
        Node::Element(el) => {
            if el.attributes().is_empty() && el.children.is_empty() {
                return false;
            }

            // also doesn't work if the top-level element is a MathML element
            let el_name = el.name().to_string();
            if is_math_ml_element(&el_name) {
                return false;
            }
        }
        _ => return false,
    }

    // otherwise, walk over all the nodes to make sure everything is inert
    let mut nodes = VecDeque::from([orig_node]);

    while let Some(current_element) = nodes.pop_front() {
        match current_element {
            Node::Text(_) | Node::RawText(_) => {}
            Node::Element(node) => {
                if is_component_node(node) {
                    return false;
                }
                if is_spread_marker(node) {
                    return false;
                }

                match node.name() {
                    NodeName::Block(_) => return false,
                    _ => {
                        // check all attributes
                        for attr in node.attributes() {
                            match attr {
                                NodeAttribute::Block(_) => return false,
                                NodeAttribute::Attribute(attr) => {
                                    let static_key = !matches!(attr.key, NodeName::Block(_));

                                    let static_value = match attr.possible_value.to_value() {
                                        None => true,
                                        Some(value) => {
                                            matches!(&value.value, KVAttributeValue::Expr(expr) if {
                                                if let Expr::Lit(lit) = expr {
                                                    let key = attr.key.to_string();
                                                    if key.starts_with("style:") || key.starts_with("prop:") || key.starts_with("on:") || key.starts_with("use:") || key.starts_with("bind") {
                                                        false
                                                    } else {
                                                        matches!(&lit.lit, Lit::Str(_))
                                                    }
                                                } else {
                                                    false
                                                }
                                            })
                                        }
                                    };

                                    if !static_key || !static_value {
                                        return false;
                                    }
                                }
                            }
                        }

                        // check all children
                        nodes.extend(&node.children);
                    }
                }
            }
            _ => return false,
        }
    }

    true
}

enum Item<'a, T> {
    Node(&'a Node<T>, bool),
    ClosingTag(String),
}

enum InertElementBuilder<'a> {
    GlobalClass {
        global_class: &'a TokenTree,
        strs: Vec<GlobalClassItem<'a>>,
        buffer: String,
    },
    NoGlobalClass {
        buffer: String,
    },
}

impl ToTokens for InertElementBuilder<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            InertElementBuilder::GlobalClass { strs, .. } => {
                tokens.extend(quote! {
                    [#(#strs),*].join("")
                });
            }
            InertElementBuilder::NoGlobalClass { buffer } => tokens.extend(quote! {
                #buffer
            }),
        }
    }
}

enum GlobalClassItem<'a> {
    Global(&'a TokenTree),
    String(String),
}

impl ToTokens for GlobalClassItem<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let addl_tokens = match self {
            GlobalClassItem::Global(v) => v.to_token_stream(),
            GlobalClassItem::String(v) => v.to_token_stream(),
        };
        tokens.extend(addl_tokens);
    }
}

impl<'a> InertElementBuilder<'a> {
    fn new(global_class: Option<&'a TokenTree>) -> Self {
        match global_class {
            None => Self::NoGlobalClass {
                buffer: String::new(),
            },
            Some(global_class) => Self::GlobalClass {
                global_class,
                strs: Vec::new(),
                buffer: String::new(),
            },
        }
    }

    fn push(&mut self, c: char) {
        match self {
            InertElementBuilder::GlobalClass { buffer, .. } => buffer.push(c),
            InertElementBuilder::NoGlobalClass { buffer } => buffer.push(c),
        }
    }

    fn push_str(&mut self, s: &str) {
        match self {
            InertElementBuilder::GlobalClass { buffer, .. } => buffer.push_str(s),
            InertElementBuilder::NoGlobalClass { buffer } => buffer.push_str(s),
        }
    }

    fn push_class(&mut self, class: &str) {
        match self {
            InertElementBuilder::GlobalClass {
                global_class,
                strs,
                buffer,
            } => {
                buffer.push_str(" class=\"");
                strs.push(GlobalClassItem::String(std::mem::take(buffer)));
                strs.push(GlobalClassItem::Global(global_class));
                buffer.push(' ');
                buffer.push_str(class);
                buffer.push('"');
            }
            InertElementBuilder::NoGlobalClass { buffer } => {
                buffer.push_str(" class=\"");
                buffer.push_str(class);
                buffer.push('"');
            }
        }
    }

    fn finish(&mut self) {
        match self {
            InertElementBuilder::GlobalClass { strs, buffer, .. } => {
                strs.push(GlobalClassItem::String(std::mem::take(buffer)));
            }
            InertElementBuilder::NoGlobalClass { .. } => {}
        }
    }
}

fn inert_element_to_tokens(
    node: &Node<impl CustomNode>,
    escape_text: bool,
    global_class: Option<&TokenTree>,
) -> TokenStream {
    let mut html = InertElementBuilder::new(global_class);
    let mut nodes = VecDeque::from([Item::Node(node, escape_text)]);

    while let Some(current) = nodes.pop_front() {
        match current {
            Item::ClosingTag(tag) => {
                // closing tag
                html.push_str("</");
                html.push_str(&tag);
                html.push('>');
            }
            Item::Node(current, escape) => {
                match current {
                    Node::RawText(raw) => {
                        let text = raw.to_string_best();
                        let text = if escape {
                            html_escape::encode_text(&text)
                        } else {
                            text.into()
                        };
                        html.push_str(&text);
                    }
                    Node::Text(text) => {
                        let text = text.value_string();
                        let text = if escape {
                            html_escape::encode_text(&text)
                        } else {
                            text.into()
                        };
                        html.push_str(&text);
                    }
                    Node::Element(node) => {
                        let self_closing = is_self_closing(node);
                        let el_name = node.name().to_string();
                        let escape = el_name != "script" && el_name != "style";

                        // opening tag
                        html.push('<');
                        html.push_str(&el_name);

                        for attr in node.attributes() {
                            if let NodeAttribute::Attribute(attr) = attr {
                                let attr_name = attr.key.to_string();
                                // trim r# from raw identifiers like r#as
                                let attr_name = attr_name.trim_start_matches("r#");
                                if attr_name != "class" {
                                    html.push(' ');
                                    html.push_str(attr_name);
                                }

                                if let Some(value) = attr.possible_value.to_value() {
                                    if let KVAttributeValue::Expr(Expr::Lit(lit)) = &value.value {
                                        if let Lit::Str(txt) = &lit.lit {
                                            let value = txt.value();
                                            let value =
                                                html_escape::encode_double_quoted_attribute(&value);
                                            if attr_name == "class" {
                                                html.push_class(&value);
                                            } else {
                                                html.push_str("=\"");
                                                html.push_str(&value);
                                                html.push('"');
                                            }
                                        }
                                    }
                                };
                            }
                        }

                        html.push('>');

                        // render all children
                        if !self_closing {
                            nodes.push_front(Item::ClosingTag(el_name));
                            let children = node.children.iter().rev();
                            for child in children {
                                nodes.push_front(Item::Node(child, escape));
                            }
                        }
                    }
                    Node::Comment(_)
                    | Node::Doctype(_)
                    | Node::Block(_)
                    | Node::Fragment(_)
                    | Node::Custom(_) => {}
                }
            }
        }
    }

    html.finish();

    quote! {
        ::web_interface::view_renderer::html::InertElement::new(#html)
    }
}

/// # Note
/// Should not be used on top level `<svg>` elements.
/// Use [`inert_element_to_tokens`] instead.
fn inert_svg_element_to_tokens(
    node: &Node<impl CustomNode>,
    escape_text: bool,
    global_class: Option<&TokenTree>,
) -> TokenStream {
    let mut html = InertElementBuilder::new(global_class);
    let mut nodes = VecDeque::from([Item::Node(node, escape_text)]);

    while let Some(current) = nodes.pop_front() {
        match current {
            Item::ClosingTag(tag) => {
                // closing tag
                html.push_str("</");
                html.push_str(&tag);
                html.push('>');
            }
            Item::Node(current, escape) => {
                match current {
                    Node::RawText(raw) => {
                        let text = raw.to_string_best();
                        let text = if escape {
                            html_escape::encode_text(&text)
                        } else {
                            text.into()
                        };
                        html.push_str(&text);
                    }
                    Node::Text(text) => {
                        let text = text.value_string();
                        let text = if escape {
                            html_escape::encode_text(&text)
                        } else {
                            text.into()
                        };
                        html.push_str(&text);
                    }
                    Node::Element(node) => {
                        let self_closing = is_self_closing(node);
                        let el_name = node.name().to_string();
                        // strip trailing underscores, for identifiers such as SVG use_
                        let el_name = el_name
                            .strip_suffix('_')
                            .map(str::to_string)
                            .unwrap_or(el_name);

                        let escape = el_name != "script" && el_name != "style";

                        // opening tag
                        html.push('<');
                        html.push_str(&el_name);

                        for attr in node.attributes() {
                            if let NodeAttribute::Attribute(attr) = attr {
                                let attr_name = attr.key.to_string();
                                // trim r# from raw identifiers like r#as
                                let attr_name = attr_name.trim_start_matches("r#");
                                if attr_name != "class" {
                                    html.push(' ');
                                    html.push_str(attr_name);
                                }

                                if let Some(value) = attr.possible_value.to_value() {
                                    if let KVAttributeValue::Expr(Expr::Lit(lit)) = &value.value {
                                        if let Lit::Str(txt) = &lit.lit {
                                            let value = txt.value();
                                            let value =
                                                html_escape::encode_double_quoted_attribute(&value);
                                            if attr_name == "class" {
                                                html.push_class(&value);
                                            } else {
                                                html.push_str("=\"");
                                                html.push_str(&value);
                                                html.push('"');
                                            }
                                        }
                                    }
                                };
                            }
                        }

                        html.push('>');

                        // render all children
                        if !self_closing {
                            nodes.push_front(Item::ClosingTag(el_name));
                            let children = node.children.iter().rev();
                            for child in children {
                                nodes.push_front(Item::Node(child, escape));
                            }
                        }
                    }
                    Node::Comment(_)
                    | Node::Doctype(_)
                    | Node::Block(_)
                    | Node::Fragment(_)
                    | Node::Custom(_) => {}
                }
            }
        }
    }

    html.finish();

    quote! {
        ::web_interface::view_renderer::svg::InertElement::new(#html)
    }
}

fn element_children_to_tokens(
    nodes: &mut [Node<impl CustomNode>],
    parent_type: TagType,
    parent_slots: Option<&mut HashMap<String, Vec<TokenStream>>>,
    global_class: Option<&TokenTree>,
    view_marker: Option<&str>,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Option<TokenStream>> {
    let children = children_to_tokens(
        nodes,
        parent_type,
        parent_slots,
        global_class,
        view_marker,
        false,
        disable_inert_html,
        errors,
    )?;
    Ok(if children.is_empty() {
        None
    } else if children.len() == 1 {
        let child = &children[0];
        Some(quote! {
            .child(
                #[allow(unused_braces)]
                { #child }
            )
        })
    } else if children.len() > 16 {
        // implementations of various traits used in routing and rendering are implemented for
        // tuples of sizes 0, 1, 2, 3, ... N. N varies but is > 16. The traits are also implemented
        // for tuples of tuples, so if we have more than 16 items, we can split them out into
        // multiple tuples.
        let chunks = children.chunks(16).map(|children| {
            quote! {
                (#(#children),*)
            }
        });
        Some(quote! {
            .child(
                (#(#chunks),*)
            )
        })
    } else {
        Some(quote! {
            .child(
                (#(#children),*)
            )
        })
    })
}

fn fragment_to_tokens(
    nodes: &mut [Node<impl CustomNode>],
    parent_type: TagType,
    parent_slots: Option<&mut HashMap<String, Vec<TokenStream>>>,
    global_class: Option<&TokenTree>,
    view_marker: Option<&str>,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Option<TokenStream>> {
    let children = children_to_tokens(
        nodes,
        parent_type,
        parent_slots,
        global_class,
        view_marker,
        true,
        disable_inert_html,
        errors,
    )?;
    Ok(if children.is_empty() {
        None
    } else if children.len() == 1 {
        children.into_iter().next()
    } else if children.len() > 16 {
        // implementations of various traits used in routing and rendering are implemented for
        // tuples of sizes 0, 1, 2, 3, ... N. N varies but is > 16. The traits are also implemented
        // for tuples of tuples, so if we have more than 16 items, we can split them out into
        // multiple tuples.
        let chunks = children.chunks(16).map(|children| {
            quote! {
                (#(#children),*)
            }
        });
        Some(quote! {
             (#(#chunks),*)
        })
    } else {
        Some(quote! {
            (#(#children),*)
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn children_to_tokens(
    nodes: &mut [Node<impl CustomNode>],
    parent_type: TagType,
    parent_slots: Option<&mut HashMap<String, Vec<TokenStream>>>,
    global_class: Option<&TokenTree>,
    view_marker: Option<&str>,
    top_level: bool,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Vec<TokenStream>> {
    if nodes.len() == 1 {
        match node_to_tokens(
            &mut nodes[0],
            parent_type,
            parent_slots,
            global_class,
            view_marker,
            top_level,
            disable_inert_html,
            errors,
        )? {
            Some(tokens) => Ok(vec![tokens]),
            None => Ok(vec![]),
        }
    } else {
        let mut slots = HashMap::new();
        let nodes = nodes
            .iter_mut()
            .filter_map(|node| {
                node_to_tokens(
                    node,
                    TagType::Unknown,
                    Some(&mut slots),
                    global_class,
                    view_marker,
                    top_level,
                    disable_inert_html,
                    errors,
                )
                .transpose()
            })
            .collect::<rust_syntax::Result<Vec<_>>>()?;
        if let Some(parent_slots) = parent_slots {
            for (slot, mut values) in slots.drain() {
                parent_slots
                    .entry(slot)
                    .and_modify(|entry| entry.append(&mut values))
                    .or_insert(values);
            }
        }
        Ok(nodes)
    }
}

#[allow(clippy::too_many_arguments)]
fn node_to_tokens(
    node: &mut Node<impl CustomNode>,
    parent_type: TagType,
    parent_slots: Option<&mut HashMap<String, Vec<TokenStream>>>,
    global_class: Option<&TokenTree>,
    view_marker: Option<&str>,
    top_level: bool,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Option<TokenStream>> {
    let is_inert = !disable_inert_html && is_inert_element(node);

    match node {
        Node::Comment(_) => Ok(None),
        Node::Doctype(node) => Err(rust_syntax::Error::new(
            node.span(),
            "browser views cannot create document doctypes",
        )),
        Node::Fragment(fragment) => fragment_to_tokens(
            &mut fragment.children,
            parent_type,
            parent_slots,
            global_class,
            view_marker,
            disable_inert_html,
            errors,
        ),
        Node::Block(block) => Ok(Some(
            quote! { ::web_interface::prelude::IntoRender::into_render(#block) },
        )),
        Node::Text(text) => Ok(Some(text_to_tokens(&text.value))),
        Node::RawText(raw) => {
            let text = raw.to_string_best();
            let text = rust_syntax::LitStr::new(&text, raw.span());
            Ok(Some(text_to_tokens(&text)))
        }
        Node::Element(el_node) => {
            if !top_level && is_inert {
                let el_name = el_node.name().to_string();
                let escape = el_name != "script" && el_name != "style";

                let el_name = el_node.name().to_string();
                if is_svg_element(&el_name) && el_name != "svg" {
                    Ok(Some(inert_svg_element_to_tokens(
                        node,
                        escape,
                        global_class,
                    )))
                } else {
                    Ok(Some(inert_element_to_tokens(node, escape, global_class)))
                }
            } else {
                element_to_tokens(
                    el_node,
                    parent_type,
                    parent_slots,
                    global_class,
                    view_marker,
                    disable_inert_html,
                    errors,
                )
            }
        }
        Node::Custom(node) => Ok(Some(node.to_token_stream())),
    }
}

fn text_to_tokens(text: &LitStr) -> TokenStream {
    quote! { #text }
}

fn attribute_sort_priority(attribute: &NodeAttribute) -> u8 {
    if let NodeAttribute::Attribute(attribute) = attribute {
        if matches!(attribute.value(), Some(Tuple(_))) {
            return 2;
        }
        if let NodeName::Path(path) = &attribute.key {
            if path
                .path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == "class" || segment.ident == "style")
            {
                return 0;
            }
        }
    }
    1
}

pub(crate) fn element_to_tokens(
    node: &mut NodeElement<impl CustomNode>,
    mut parent_type: TagType,
    parent_slots: Option<&mut HashMap<String, Vec<TokenStream>>>,
    global_class: Option<&TokenTree>,
    view_marker: Option<&str>,
    disable_inert_html: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<Option<TokenStream>> {
    // attribute sorting:
    //
    // the `class` and `style` attributes overwrite individual `class:` and `style:` attributes
    // when they are set. as a result, we're going to sort the attributes so that `class` and
    // `style` always come before all other attributes.

    // if there's a spread marker, we don't want to move `class` or `style` before it
    // so let's only sort attributes that come *before* a spread marker
    let spread_position = node
        .attributes()
        .iter()
        .position(|n| match n {
            NodeAttribute::Block(node) => as_spread_attr(node).is_some(),
            _ => false,
        })
        .unwrap_or_else(|| node.attributes().len());

    // A stable key is a total order: class and style share priority, as do
    // tuple additions. A comparator returning Less for both class/style orders
    // (or Greater for both tuple orders) can panic and reverse sibling setters.
    node.attributes_mut()[0..spread_position].sort_by_key(attribute_sort_priority);

    // check for duplicate attribute names and emit an error for all subsequent ones
    let mut names = HashSet::new();

    // allow multiple class=(...) or style=(...) attributes
    fn allow_multiples(name: &str, attr: &KeyedAttribute) -> bool {
        (name == "class" || name == "style") && matches!(attr.value(), Some(Expr::Tuple(..)))
    }

    for attr in node.attributes() {
        if let NodeAttribute::Attribute(attr) = attr {
            let mut name = attr.key.to_string();
            match tuple_name(&name, attr)? {
                TupleName::None => {}
                TupleName::Str(tuple_name) => {
                    name.push(':');
                    name.push_str(&tuple_name);
                }
                TupleName::Array(names) => {
                    for tuple_name in names {
                        name.push(':');
                        name.push_str(&tuple_name);
                    }
                }
            }
            if names.contains(&name) && !allow_multiples(&name, attr) {
                errors.push(rust_syntax::Error::new_spanned(
                    attr,
                    format!("This element already has a `{name}` attribute."),
                ));
            } else {
                names.insert(name);
            }
        }
    }

    let name = node.name();
    if is_component_node(node) {
        if let Some(slot) = get_slot(node) {
            let slot = slot.clone();
            slot_to_tokens(
                node,
                &slot,
                parent_slots,
                global_class,
                disable_inert_html,
                errors,
            )?;
            Ok(None)
        } else {
            component_to_tokens(node, global_class, disable_inert_html, errors).map(Some)
        }
    } else if is_spread_marker(node) {
        let mut attributes = Vec::new();
        let mut additions = Vec::new();
        for node in node.attributes() {
            match node {
                NodeAttribute::Block(block) => {
                    if let NodeBlock::ValidBlock(block) = block {
                        match block.stmts.first() {
                            Some(Stmt::Expr(
                                Expr::Range(ExprRange {
                                    start: None,
                                    limits: RangeLimits::HalfOpen(_),
                                    end: Some(end),
                                    ..
                                }),
                                _,
                            )) => {
                                additions.push(quote! { #end });
                            }
                            _ => {
                                additions.push(quote! { #block });
                            }
                        }
                    } else {
                        additions.push(quote! { #block });
                    }
                }
                NodeAttribute::Attribute(node) => {
                    if let Some(content) = attribute_absolute(node, true)? {
                        attributes.push(content);
                    }
                }
            }
        }

        Ok(Some(quote! {
            (#(#attributes,)*)
            #(.add_any_attr(#additions))*
        }))
    } else {
        let tag = name.to_string();
        let is_custom = is_custom_element(&tag);
        let name = if is_custom {
            let name = node.name().to_string();
            // link custom ident to name span for IDE docs
            let custom = Ident::new("custom", name.span());
            quote_spanned! { node.name().span() => ::web_interface::view_renderer::html::element::#custom(#name) }
        } else if is_svg_element(&tag) {
            parent_type = TagType::Svg;
            let name = if tag == "use" || tag == "use_" {
                Ident::new_raw("use", name.span()).to_token_stream()
            } else {
                name.to_token_stream()
            };
            quote_spanned! { node.name().span() => ::web_interface::view_renderer::svg::#name() }
        } else if is_math_ml_element(&tag) {
            return Err(rust_syntax::Error::new(
                node.name().span(),
                "typed MathML elements are not supported by this browser interface",
            ));
        } else if is_ambiguous_element(&tag) {
            match parent_type {
                TagType::Unknown => {
                    // We decided this warning was too aggressive.
                    quote_spanned! { node.name().span() =>
                        ::web_interface::view_renderer::html::element::#name()
                    }
                }
                TagType::Html => {
                    quote_spanned! { node.name().span() => ::web_interface::view_renderer::html::element::#name() }
                }
                TagType::Svg => {
                    quote_spanned! { node.name().span() => ::web_interface::view_renderer::svg::#name() }
                }
            }
        } else {
            parent_type = TagType::Html;
            quote_spanned! { name.span() => ::web_interface::view_renderer::html::element::#name() }
        };

        let attributes = node.attributes();
        let attributes = if attributes.len() == 1 {
            Some(attribute_to_tokens(
                parent_type,
                &attributes[0],
                global_class,
                is_custom,
                errors,
            )?)
        } else {
            let nodes = attributes
                .iter()
                .map(|node| attribute_to_tokens(parent_type, node, global_class, is_custom, errors))
                .collect::<rust_syntax::Result<Vec<_>>>()?;
            Some(quote! {
                #(#nodes)*
            })
        };

        let global_class_expr = global_class.map(|class| {
            quote! { .class((#class, true)) }
        });

        let self_closing = is_self_closing(node);
        let children = if !self_closing {
            element_children_to_tokens(
                &mut node.children,
                parent_type,
                parent_slots,
                global_class,
                view_marker,
                disable_inert_html,
                errors,
            )?
        } else {
            if !node.children.is_empty() {
                let name = node.name();
                errors.push(rust_syntax::Error::new_spanned(
                    name,
                    format!(
                        "Self-closing elements like <{name}> cannot have \
                         children."
                    ),
                ));
            };
            None
        };

        // attributes are placed second because this allows `inner_html`
        // to object if there are already children
        Ok(Some(quote! {
            #name
            #children
            #attributes
            #global_class_expr
        }))
    }
}

fn is_spread_marker(node: &NodeElement<impl CustomNode>) -> bool {
    match node.name() {
        NodeName::Block(block) => matches!(
            block.stmts.first(),
            Some(Stmt::Expr(
                Expr::Range(ExprRange {
                    start: None,
                    limits: RangeLimits::HalfOpen(_),
                    end: None,
                    ..
                }),
                _,
            ))
        ),
        _ => false,
    }
}

fn as_spread_attr(node: &NodeBlock) -> Option<Option<&Expr>> {
    if let NodeBlock::ValidBlock(block) = node {
        match block.stmts.first() {
            Some(Stmt::Expr(
                Expr::Range(ExprRange {
                    start: None,
                    limits: RangeLimits::HalfOpen(_),
                    end,
                    ..
                }),
                _,
            )) => Some(end.as_deref()),
            _ => None,
        }
    } else {
        None
    }
}

fn attribute_to_tokens(
    tag_type: TagType,
    node: &NodeAttribute,
    global_class: Option<&TokenTree>,
    is_custom: bool,
    errors: &mut Errors,
) -> rust_syntax::Result<TokenStream> {
    Ok(match node {
        NodeAttribute::Block(node) => as_spread_attr(node)
            .flatten()
            .map(|end| {
                quote! {
                    .add_any_attr(#end)
                }
            })
            .unwrap_or_else(|| {
                quote! {
                    .add_any_attr(#[allow(unused_braces)] { #node })
                }
            }),
        NodeAttribute::Attribute(node) => {
            let name = node.key.to_string();
            if name == "node_ref" {
                let node_ref = match &node.key {
                    NodeName::Path(path) => path.path.get_ident(),
                    NodeName::Punctuated(_) | NodeName::Block(_) => {
                        unreachable!("plain attribute names are parsed as paths")
                    }
                };
                let value = attribute_value(node, false);
                quote! {
                    .#node_ref(#value)
                }
            } else if let Some(name) = name.strip_prefix("use:") {
                directive_call_from_attribute_node(node, name)
            } else if let Some(name) = name.strip_prefix("on:") {
                event_to_tokens(name, node)?
            } else if let Some(name) = name.strip_prefix("bind:") {
                two_way_binding_to_tokens(name, node)
            } else if let Some(name) = name.strip_prefix("class:") {
                let class = match &node.key {
                    NodeName::Punctuated(parts) => &parts[0],
                    NodeName::Path(_) | NodeName::Block(_) => {
                        unreachable!("prefixed attribute names are punctuated")
                    }
                };
                class_to_tokens(node, class.into_token_stream(), Some(name))?
            } else if name == "class" {
                let class = match &node.key {
                    NodeName::Path(path) => path.path.get_ident(),
                    NodeName::Punctuated(_) | NodeName::Block(_) => {
                        unreachable!("plain attribute names are parsed as paths")
                    }
                };
                class_to_tokens(node, class.into_token_stream(), None)?
            } else if let Some(name) = name.strip_prefix("style:") {
                let style = match &node.key {
                    NodeName::Punctuated(parts) => &parts[0],
                    NodeName::Path(_) | NodeName::Block(_) => {
                        unreachable!("prefixed attribute names are punctuated")
                    }
                };
                style_to_tokens(node, style.into_token_stream(), Some(name))
            } else if name == "style" {
                let style = match &node.key {
                    NodeName::Path(path) => path.path.get_ident(),
                    NodeName::Punctuated(_) | NodeName::Block(_) => {
                        unreachable!("plain attribute names are parsed as paths")
                    }
                };
                style_to_tokens(node, style.into_token_stream(), None)
            } else if let Some(name) = name.strip_prefix("prop:") {
                let prop = match &node.key {
                    NodeName::Punctuated(parts) => &parts[0],
                    NodeName::Path(_) | NodeName::Block(_) => {
                        unreachable!("prefixed attribute names are punctuated")
                    }
                };
                prop_to_tokens(node, prop.into_token_stream(), name)
            }
            // circumstances in which we just do unchecked attributes
            // 1) custom elements, which can have any attributes
            // 2) custom attributes and data attributes (so, anything with - in it)
            else if is_custom ||
                (name.contains('-') && !name.starts_with("aria-"))
                // TODO(web-interface): audit view_renderer SVG setters before replacing this
                // unchecked SVG-attribute fallback with typed methods.
                || (tag_type == TagType::Svg && name != "inner_html")
            {
                let value = attribute_value(node, true);
                quote! {
                    .attr(#name, #value)
                }
            } else {
                let key = attribute_name(&node.key);
                let value = attribute_value(node, true);

                // special case of global_class and class attribute
                if &node.key.to_string() == "class"
                    && global_class.is_some()
                    && node.value().and_then(value_to_string).is_none()
                {
                    let span = node.key.span();
                    errors.push(rust_syntax::Error::new(span, "Combining a global class (view! { class = ... }) \
            and a dynamic `class=` attribute on an element causes runtime inconsistencies. You can \
            toggle individual classes dynamically with the `class:name=value` syntax. \n\nSee this issue \
            for more information and an example: docs/components/interface.md"));
                };

                quote! {
                    .#key(#value)
                }
            }
        }
    })
}

/// Returns attribute values with an absolute path
pub(crate) fn attribute_absolute(
    node: &KeyedAttribute,
    after_spread: bool,
) -> rust_syntax::Result<Option<TokenStream>> {
    let key = node.key.to_string();
    let contains_dash = key.contains('-');
    let attr_colon = key.starts_with("attr:")
        || key.starts_with("style:")
        || key.starts_with("class:")
        || key.starts_with("prop:")
        || key.starts_with("use:");
    // anything that follows the x:y pattern
    Ok(match &node.key {
        NodeName::Punctuated(parts) if !contains_dash || attr_colon => {
            if parts.len() >= 2 {
                let id = &parts[0];
                match id {
                    NodeNameFragment::Ident(id) => {
                        // ignore `let:` and `clone:`
                        if id == "let" || id == "clone" {
                            None
                        } else if id == "attr" {
                            let value = attribute_value(node, true);
                            let multipart = parts.len() > 2;
                            let key = &parts[1];
                            let key_name = key.to_string();
                            if key_name == "class" || key_name == "style" {
                                Some(quote! { ::web_interface::view_renderer::html::#key::#key(#value) })
                            } else if key_name == "aria" {
                                let value = attribute_value(node, true);
                                let mut parts_iter = parts.iter();
                                parts_iter.next();
                                let fn_name = parts_iter
                                    .map(|p| p.to_string())
                                    .collect::<Vec<String>>()
                                    .join("_");
                                let key = Ident::new(&fn_name, key.span());
                                Some(quote! { ::web_interface::view_renderer::html::attribute::#key(#value) })
                            } else if multipart {
                                // e.g., attr:data-foo="bar"
                                let key_name = parts
                                    .pairs()
                                    .skip(1)
                                    .map(|p| match p {
                                        Punctuated(n, p) => format!("{n}{p}"),
                                        End(n) => n.to_string(),
                                    })
                                    .collect::<String>();
                                Some(
                                    quote! { ::web_interface::view_renderer::html::attribute::custom::custom_attribute(#key_name, #value) },
                                )
                            } else {
                                Some(quote! { ::web_interface::view_renderer::html::attribute::#key(#value) })
                            }
                        } else if id == "use" {
                            let key = &parts[1];
                            let param = if let Some(value) = node.value() {
                                quote!(#value)
                            } else {
                                quote_spanned!(node.key.span()=> ().into())
                            };
                            Some(quote! {
                                ::web_interface::view_renderer::html::directive::directive(
                                    #key,
                                    #[allow(clippy::useless_conversion)] #param
                                )
                            })
                        } else if id == "style" || id == "class" {
                            let value = attribute_value(node, false);
                            let key = &node.key.to_string();
                            let key = key.replacen("style:", "", 1).replacen("class:", "", 1);
                            Some(quote! { ::web_interface::view_renderer::html::#id::#id((#key, #value)) })
                        } else if id == "prop" {
                            let value = attribute_value(node, false);
                            let key = &node.key.to_string();
                            let key = key.replacen("prop:", "", 1);
                            Some(quote! { ::web_interface::view_renderer::html::property::#id(#key, #value) })
                        } else if id == "on" {
                            let key = &node.key.to_string();
                            let key = key.replacen("on:", "", 1);
                            let (on, ty, handler) = event_type_and_handler(&key, node)?;
                            Some(quote! { ::web_interface::view_renderer::html::event::#on(#ty, #handler) })
                        } else {
                            return Err(rust_syntax::Error::new(
                                id.span(),
                                format!(
                                    "`{id}:` syntax is not supported on \
                                     components"
                                ),
                            ));
                        }
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => after_spread.then(|| {
            let key = attribute_name(&node.key);
            let value = &node.value();
            let name = &node.key.to_string();
            if name == "class" || name == "style" {
                quote! {
                    ::web_interface::view_renderer::html::#key::#key(#value)
                }
            } else if name.contains('-') && !name.starts_with("aria-") {
                quote! {
                    ::web_interface::view_renderer::html::attribute::custom::custom_attribute(#name, #value)
                }
            } else if name == "node_ref" {
                quote! {
                    ::web_interface::view_renderer::html::node_ref::#key(#value)
                }
            } else {
                quote! {
                    ::web_interface::view_renderer::html::attribute::#key(#value)
                }
            }
        }),
    })
}

pub(crate) fn two_way_binding_to_tokens(name: &str, node: &KeyedAttribute) -> TokenStream {
    let value = attribute_value(node, false);

    let ident = format_ident!("{}", name.to_case(UpperCamel), span = node.key.span());

    if name == "group" {
        quote! {
            .bind(web_interface::view_renderer::reactive_state::bind::#ident, #value)
        }
    } else {
        quote! {
            .bind(::web_interface::attr::#ident, #value)
        }
    }
}

pub(crate) fn event_to_tokens(
    name: &str,
    node: &KeyedAttribute,
) -> rust_syntax::Result<TokenStream> {
    let (on, event_type, handler) = event_type_and_handler(name, node)?;

    Ok(quote! {
        .#on(#event_type, #handler)
    })
}

pub(crate) fn event_type_and_handler(
    name: &str,
    node: &KeyedAttribute,
) -> rust_syntax::Result<(TokenStream, TokenStream, TokenStream)> {
    let handler = attribute_value(node, false);

    let (event_type, is_custom, options) = parse_event_name(name);

    let parts = match &node.key {
        NodeName::Punctuated(parts) => parts,
        NodeName::Path(_) | NodeName::Block(_) => {
            unreachable!("event attributes have a punctuated on:event name")
        }
    };
    let event_name_ident = parts.iter().nth(1);
    let undelegated_ident = parts.iter().find(|part| part.to_string() == "undelegated");
    let capture_ident = parts.iter().find(|part| part.to_string() == "capture");
    let on = &parts[0];
    let on = if options.targeted {
        Ident::new("on_target", on.span()).to_token_stream()
    } else {
        on.to_token_stream()
    };
    let event_type = if is_custom {
        event_type
    } else if let Some(ev_name) = event_name_ident {
        quote! { #ev_name }
    } else {
        event_type
    };

    let event_type = quote! {
        ::web_interface::view_renderer::html::event::#event_type
    };
    let event_type = if options.captured {
        let capture = if let Some(capture) = capture_ident {
            quote! { #capture }
        } else {
            quote! { capture }
        };
        quote! { ::web_interface::view_renderer::html::event::#capture(#event_type) }
    } else {
        event_type
    };

    let event_type = if options.undelegated {
        let undelegated = if let Some(undelegated) = undelegated_ident {
            quote! { #undelegated }
        } else {
            quote! { undelegated }
        };
        quote! { ::web_interface::view_renderer::html::event::#undelegated(#event_type) }
    } else {
        event_type
    };

    Ok((on, event_type, handler))
}

fn class_to_tokens(
    node: &KeyedAttribute,
    class: TokenStream,
    class_name: Option<&str>,
) -> rust_syntax::Result<TokenStream> {
    // case of class=(["foo", "bar"], /* something */)
    // just expands to multiple uses of class:
    if let Some(Tuple(tuple)) = node.value() {
        if tuple.elems.len() == 2 {
            let name = &tuple.elems[0];
            let value = &tuple.elems[1];
            if let Expr::Array(ExprArray { elems, .. }) = name {
                return elems
                    .iter()
                    .map(|elem| match elem {
                        Expr::Lit(ExprLit {
                            lit: Lit::Str(s), ..
                        }) => Ok(quote! {
                            .#class((#s, #value))
                        }),
                        _ => Err(rust_syntax::Error::new_spanned(elem, "invalid name")),
                    })
                    .collect();
            }
        }
    }

    // default case
    let value = attribute_value(node, false);
    if let Some(class_name) = class_name {
        Ok(quote! {
            .#class((#class_name, #value))
        })
    } else {
        Ok(quote! {
            .#class(#value)
        })
    }
}

fn style_to_tokens(
    node: &KeyedAttribute,
    style: TokenStream,
    style_name: Option<&str>,
) -> TokenStream {
    let value = attribute_value(node, false);
    if let Some(style_name) = style_name {
        quote! {
            .#style((#style_name, #value))
        }
    } else {
        quote! {
            .#style(#value)
        }
    }
}

fn prop_to_tokens(node: &KeyedAttribute, prop: TokenStream, key: &str) -> TokenStream {
    let value = attribute_value(node, false);
    quote! {
        .#prop(#key, #value)
    }
}

fn is_custom_element(tag: &str) -> bool {
    tag.contains('-')
}

fn is_self_closing(node: &NodeElement<impl CustomNode>) -> bool {
    // self-closing tags
    // https://developer.mozilla.org/en-US/docs/Glossary/Empty_element
    [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ]
    .binary_search(&node.name().to_string().as_str())
    .is_ok()
}

fn is_svg_element(tag: &str) -> bool {
    // Keep list alphabetized for binary search
    [
        "animate",
        "animateMotion",
        "animateTransform",
        "circle",
        "clipPath",
        "defs",
        "desc",
        "discard",
        "ellipse",
        "feBlend",
        "feColorMatrix",
        "feComponentTransfer",
        "feComposite",
        "feConvolveMatrix",
        "feDiffuseLighting",
        "feDisplacementMap",
        "feDistantLight",
        "feDropShadow",
        "feFlood",
        "feFuncA",
        "feFuncB",
        "feFuncG",
        "feFuncR",
        "feGaussianBlur",
        "feImage",
        "feMerge",
        "feMergeNode",
        "feMorphology",
        "feOffset",
        "fePointLight",
        "feSpecularLighting",
        "feSpotLight",
        "feTile",
        "feTurbulence",
        "filter",
        "foreignObject",
        "g",
        "hatch",
        "hatchpath",
        "image",
        "line",
        "linearGradient",
        "marker",
        "mask",
        "metadata",
        "mpath",
        "path",
        "pattern",
        "polygon",
        "polyline",
        "radialGradient",
        "rect",
        "set",
        "stop",
        "svg",
        "switch",
        "symbol",
        "text",
        "textPath",
        "tspan",
        "use",
        "use_",
        "view",
    ]
    .binary_search(&tag)
    .is_ok()
}

fn is_math_ml_element(tag: &str) -> bool {
    // Keep list alphabetized for binary search
    [
        "annotation",
        "maction",
        "math",
        "menclose",
        "merror",
        "mfenced",
        "mfrac",
        "mi",
        "mmultiscripts",
        "mn",
        "mo",
        "mover",
        "mpadded",
        "mphantom",
        "mprescripts",
        "mroot",
        "mrow",
        "ms",
        "mspace",
        "msqrt",
        "mstyle",
        "msub",
        "msubsup",
        "msup",
        "mtable",
        "mtd",
        "mtext",
        "mtr",
        "munder",
        "munderover",
        "semantics",
    ]
    .binary_search(&tag)
    .is_ok()
}

fn is_ambiguous_element(tag: &str) -> bool {
    tag == "a" || tag == "script" || tag == "title"
}

fn parse_event(event_name: &str) -> (String, EventNameOptions) {
    // Modifiers are whole colon-separated segments, not substrings. In
    // particular, custom events such as `dialog:targetChanged` must retain
    // their exact event name and must not silently enable targeted handlers.
    let mut options = EventNameOptions {
        undelegated: false,
        targeted: false,
        captured: false,
    };
    let mut segments = event_name.split(':');
    let mut name = segments
        .next()
        .expect("invariant: split yields a first segment")
        .to_owned();
    for segment in segments {
        match segment {
            "undelegated" => options.undelegated = true,
            "target" => options.targeted = true,
            "capture" => options.captured = true,
            other => {
                name.push(':');
                name.push_str(other);
            }
        }
    }
    (name, options)
}

/// Escapes Rust keywords that are also HTML attribute names
/// to their raw-identifier form.
fn attribute_name(name: &NodeName) -> TokenStream {
    let s = name.to_string();
    if s == "as" || s == "async" || s == "loop" || s == "for" || s == "type" {
        Ident::new_raw(&s, name.span()).to_token_stream()
    } else if s.starts_with("aria-") {
        Ident::new(&s.replace('-', "_"), name.span()).to_token_stream()
    } else {
        name.to_token_stream()
    }
}

fn attribute_value(attr: &KeyedAttribute, is_attribute_proper: bool) -> TokenStream {
    match attr.possible_value.to_value() {
        None => quote! { true },
        Some(value) => match &value.value {
            KVAttributeValue::Expr(expr) => {
                if matches!(expr, Expr::Lit(_)) || !is_attribute_proper {
                    quote! { #expr }
                } else {
                    quote! {
                        ::web_interface::prelude::IntoAttributeValue::into_attribute_value(#expr)
                    }
                }
            }
            // any value in braces: expand as-is to give proper r-a support
            KVAttributeValue::InvalidBraced(block) => {
                if is_attribute_proper {
                    quote! {
                        ::web_interface::prelude::IntoAttributeValue::into_attribute_value(#block)
                    }
                } else {
                    quote! {
                        #block
                    }
                }
            }
        },
    }
}

// Keep list alphabetized for binary search
const TYPED_EVENTS: [&str; 127] = [
    "DOMContentLoaded",
    "abort",
    "afterprint",
    "animationcancel",
    "animationend",
    "animationiteration",
    "animationstart",
    "auxclick",
    "beforeinput",
    "beforeprint",
    "beforeunload",
    "blur",
    "canplay",
    "canplaythrough",
    "change",
    "click",
    "close",
    "compositionend",
    "compositionstart",
    "compositionupdate",
    "contextmenu",
    "copy",
    "cuechange",
    "cut",
    "dblclick",
    "devicemotion",
    "deviceorientation",
    "drag",
    "dragend",
    "dragenter",
    "dragleave",
    "dragover",
    "dragstart",
    "drop",
    "durationchange",
    "emptied",
    "ended",
    "error",
    "focus",
    "focusin",
    "focusout",
    "formdata",
    "fullscreenchange",
    "fullscreenerror",
    "gamepadconnected",
    "gamepaddisconnected",
    "gotpointercapture",
    "hashchange",
    "input",
    "invalid",
    "keydown",
    "keypress",
    "keyup",
    "languagechange",
    "load",
    "loadeddata",
    "loadedmetadata",
    "loadstart",
    "lostpointercapture",
    "message",
    "messageerror",
    "mousedown",
    "mouseenter",
    "mouseleave",
    "mousemove",
    "mouseout",
    "mouseover",
    "mouseup",
    "offline",
    "online",
    "orientationchange",
    "pagehide",
    "pageshow",
    "paste",
    "pause",
    "play",
    "playing",
    "pointercancel",
    "pointerdown",
    "pointerenter",
    "pointerleave",
    "pointerlockchange",
    "pointerlockerror",
    "pointermove",
    "pointerout",
    "pointerover",
    "pointerup",
    "popstate",
    "progress",
    "ratechange",
    "readystatechange",
    "rejectionhandled",
    "reset",
    "resize",
    "scroll",
    "scrollend",
    "securitypolicyviolation",
    "seeked",
    "seeking",
    "select",
    "selectionchange",
    "selectstart",
    "slotchange",
    "stalled",
    "storage",
    "submit",
    "suspend",
    "timeupdate",
    "toggle",
    "touchcancel",
    "touchend",
    "touchmove",
    "touchstart",
    "transitioncancel",
    "transitionend",
    "transitionrun",
    "transitionstart",
    "unhandledrejection",
    "unload",
    "visibilitychange",
    "volumechange",
    "waiting",
    "webkitanimationend",
    "webkitanimationiteration",
    "webkitanimationstart",
    "webkittransitionend",
    "wheel",
];

const CUSTOM_EVENT: &str = "Custom";

#[derive(Debug)]
pub(crate) struct EventNameOptions {
    undelegated: bool,
    targeted: bool,
    captured: bool,
}

pub(crate) fn parse_event_name(name: &str) -> (TokenStream, bool, EventNameOptions) {
    let (name, options) = parse_event(name);

    let (event_type, is_custom) = TYPED_EVENTS
        .binary_search(&name.as_str())
        .map(|_| (name.as_str(), false))
        .unwrap_or((CUSTOM_EVENT, true));

    let event_type = event_type
        .parse::<TokenStream>()
        .expect("invariant: built-in event names are Rust tokens");

    let event_type = if is_custom {
        quote! { Custom::new(#name) }
    } else {
        event_type
    };
    (event_type, is_custom, options)
}

fn convert_to_snake_case(name: String) -> String {
    name.to_case(Snake)
}

#[cfg(debug_assertions)]
pub(crate) fn ident_from_tag_name(tag_name: &NodeName, errors: &mut Errors) -> Ident {
    match tag_name {
        NodeName::Path(path) => path
            .path
            .segments
            .iter()
            .next_back()
            .map(|segment| segment.ident.clone())
            .expect("element needs to have a name"),
        NodeName::Block(_) => {
            let span = tag_name.span();
            errors.push(rust_syntax::Error::new(
                span,
                "blocks not allowed in tag-name position",
            ));
            Ident::new("", span)
        }
        _ => Ident::new(
            &tag_name.to_string().replace(['-', ':'], "_"),
            tag_name.span(),
        ),
    }
}

pub(crate) fn full_path_from_tag_name(
    tag_name: &NodeName,
    errors: &mut Errors,
) -> Option<ExprPath> {
    match tag_name {
        NodeName::Path(path) => Some(path.clone()),
        NodeName::Block(_) => {
            let span = tag_name.span();
            errors.push(rust_syntax::Error::new(
                span,
                "blocks not allowed in tag-name position",
            ));
            None
        }
        _ => {
            let span = tag_name.span();
            errors.push(rust_syntax::Error::new(
                span,
                "punctuated names not allowed in slots",
            ));
            None
        }
    }
}

pub(crate) fn directive_call_from_attribute_node(
    attr: &KeyedAttribute,
    directive_name: &str,
) -> TokenStream {
    let handler = rust_syntax::Ident::new(directive_name, attr.key.span());

    let param = if let Some(value) = attr.value() {
        quote!(#value)
    } else {
        quote_spanned!(attr.key.span()=> ().into())
    };

    quote! { .directive(#handler, #[allow(clippy::useless_conversion)] #param) }
}

fn tuple_name(name: &str, node: &KeyedAttribute) -> rust_syntax::Result<TupleName> {
    if name == "style" || name == "class" {
        if let Some(Tuple(tuple)) = node.value() {
            {
                if tuple.elems.len() == 2 {
                    let style_name = &tuple.elems[0];
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = style_name
                    {
                        return Ok(TupleName::Str(s.value()));
                    } else if let Expr::Array(ExprArray { elems, .. }) = style_name {
                        return elems
                            .iter()
                            .map(|elem| match elem {
                                Expr::Lit(ExprLit {
                                    lit: Lit::Str(s), ..
                                }) => Ok(s.value()),
                                _ => Err(rust_syntax::Error::new_spanned(elem, "invalid name")),
                            })
                            .collect::<rust_syntax::Result<Vec<_>>>()
                            .map(TupleName::Array);
                    }
                }
            }
        }
    }

    Ok(TupleName::None)
}

#[derive(Debug, PartialEq, Eq)]
enum TupleName {
    None,
    Str(String),
    Array(Vec<String>),
}

#[cfg(test)]
#[path = "../../../../test/macros/unit/view/attribute_order_tests.rs"]
mod attribute_order_tests;

#[cfg(test)]
#[path = "../../../../test/macros/unit/view/event_modifier_tests.rs"]
mod event_modifier_tests;

#[cfg(test)]
#[path = "../../../../test/macros/unit/view/case_normalization_tests.rs"]
mod case_normalization_tests;
