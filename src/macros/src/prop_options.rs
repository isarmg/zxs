//! The component and slot `prop` grammar, kept together so their shared options
//! cannot drift. Component-only options are explicitly rejected for slots.
//!
//! Diagnostic joining and merge/conflict ordering adapt the previous
//! attribute-derive / attribute-derive-macro 0.11.0 behavior. Their original
//! notices and provenance remain under compliance/; see the architecture note.

use proc_macro2::Span;
use quote::ToTokens;
use rust_syntax::{
    ext::IdentExt,
    parenthesized,
    parse::{Parse, ParseStream, Parser},
    spanned::Spanned,
    Attribute, Error, Expr, Ident, LitBool, LitStr, Meta, Result, Token,
};

#[derive(Clone, Copy)]
#[repr(usize)]
enum FieldKind {
    Optional,
    OptionalNoStrip,
    StripOption,
    DefaultValue,
    Into,
    Attrs,
    Name,
    Marker,
}

use FieldKind::*;

const FIELDS: [FieldKind; 8] = [
    Optional,
    OptionalNoStrip,
    StripOption,
    DefaultValue,
    Into,
    Attrs,
    Name,
    Marker,
];

impl FieldKind {
    fn name(self) -> &'static str {
        match self {
            Optional => "optional",
            OptionalNoStrip => "optional_no_strip",
            StripOption => "strip_option",
            DefaultValue => "default",
            Into => "into",
            Attrs => "attrs",
            Name => "name",
            Marker => "marker",
        }
    }
}

// Alphabetical order preserves deterministic diagnostics when several options
// conflict. Presence, not the boolean value, determines mutual exclusion.
const CONFLICTS: [(FieldKind, FieldKind); 10] = [
    (Attrs, Marker),
    (DefaultValue, Marker),
    (Into, Marker),
    (Marker, Name),
    (Marker, Optional),
    (Marker, OptionalNoStrip),
    (Marker, StripOption),
    (Optional, OptionalNoStrip),
    (Optional, StripOption),
    (OptionalNoStrip, StripOption),
];

#[derive(Clone, Debug, Default)]
pub(crate) struct PropOptions<const COMPONENT: bool> {
    pub(crate) optional: bool,
    pub(crate) optional_no_strip: bool,
    pub(crate) strip_option: bool,
    pub(crate) default: Option<Expr>,
    pub(crate) into: bool,
    pub(crate) attrs: bool,
    pub(crate) name: Option<String>,
    pub(crate) marker: bool,
}

enum Value {
    Flag(bool),
    Default(Box<Expr>),
    Name(String),
}

struct Field {
    name_span: Span,
    value_span: Span,
    value: Value,
}

type Fields = [Option<Field>; FIELDS.len()];

impl<const COMPONENT: bool> PropOptions<COMPONENT> {
    pub(crate) fn from_attributes(attributes: &[Attribute]) -> Result<Self> {
        let mut fields: Fields = Default::default();
        for attribute in attributes
            .iter()
            .filter(|attr| attr.path().is_ident("prop"))
        {
            let tokens = match &attribute.meta {
                Meta::Path(_) => Default::default(),
                Meta::List(list) => list.tokens.clone(),
                Meta::NameValue(value) => value.value.to_token_stream(),
            };
            // Parse each attribute first, then merge in declaration order. This
            // preserves error precedence for duplicate fields across attributes.
            let parsed = Self::parse_fields.parse2(tokens)?;
            for (index, field) in parsed.into_iter().enumerate() {
                if let Some(field) = field {
                    insert(&mut fields[index], field, FIELDS[index].name())?;
                }
            }
        }
        for (a, b) in CONFLICTS {
            if let (Some(first), Some(second)) = (&fields[a as usize], &fields[b as usize]) {
                return Err(paired_error(
                    first.name_span,
                    second.name_span,
                    format!(
                        "`{}` conflicts with mutually exclusive `{}`",
                        a.name(),
                        b.name()
                    ),
                    format!(
                        "`{}` conflicts with mutually exclusive `{}`",
                        b.name(),
                        a.name()
                    ),
                ));
            }
        }
        let mut options = Self::default();
        for (index, field) in fields.into_iter().enumerate() {
            if let Some(field) = field {
                match (FIELDS[index], field.value) {
                    (Optional, Value::Flag(value)) => options.optional = value,
                    (OptionalNoStrip, Value::Flag(value)) => options.optional_no_strip = value,
                    (StripOption, Value::Flag(value)) => options.strip_option = value,
                    (DefaultValue, Value::Default(value)) => options.default = Some(*value),
                    (Into, Value::Flag(value)) => options.into = value,
                    (Attrs, Value::Flag(value)) => options.attrs = value,
                    (Name, Value::Name(value)) => options.name = Some(value),
                    (Marker, Value::Flag(value)) => options.marker = value,
                    (DefaultValue | Name, Value::Flag(_))
                    | (
                        Optional | OptionalNoStrip | StripOption | Into | Attrs | Marker | Name,
                        Value::Default(_),
                    )
                    | (
                        Optional | OptionalNoStrip | StripOption | Into | Attrs | Marker
                        | DefaultValue,
                        Value::Name(_),
                    ) => {
                        unreachable!("field values are checked while parsing")
                    }
                }
            }
        }
        Ok(options)
    }

    fn parse_fields(input: ParseStream) -> Result<Fields> {
        let mut fields: Fields = Default::default();
        while !input.is_empty() {
            let fork = input.fork();
            let ident = Ident::parse_any(&fork).ok();
            let supported = &FIELDS[..if COMPONENT { 8 } else { 6 }];
            let Some(index) = supported
                .iter()
                .position(|name| ident.as_ref().is_some_and(|ident| ident == name.name()))
            else {
                let names = supported[..supported.len() - 1]
                    .iter()
                    .map(|name| format!("`{}`", name.name()))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(input.error(format!(
                    "supported fields are {names} and `{}`",
                    supported[supported.len() - 1].name()
                )));
            };
            let name = Ident::parse_any(input)?;
            let (value, value_span) = match FIELDS[index] {
                DefaultValue => parse_value::<Expr>(input)
                    .map(|(value, span)| (Value::Default(Box::new(value)), span))
                    .map_err(|error| value_error(error, "default", "5 * 10"))?,
                Name => {
                    let (value, span) = parse_value::<LitStr>(input)
                        .map_err(|error| value_error(error, "name", "..."))?;
                    (Value::Name(value.value()), span)
                }
                _ => {
                    if input.peek(Token![=]) || input.peek(rust_syntax::token::Paren) {
                        let (value, _) = parse_value::<LitBool>(input)
                            .map_err(|error| value_error(error, FIELDS[index].name(), "..."))?;
                        (Value::Flag(value.value), value.span())
                    } else {
                        (Value::Flag(true), name.span())
                    }
                }
            };
            insert(
                &mut fields[index],
                Field {
                    name_span: name.span(),
                    value_span,
                    value,
                },
                FIELDS[index].name(),
            )?;
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(fields)
    }
}

fn value_error(error: Error, name: &str, example: &str) -> Error {
    Error::new(
        error.span(),
        format!("{error}\n\n= help: try `#[prop({name} = {example})]`"),
    )
}

fn parse_value<T: Parse + Spanned>(input: ParseStream) -> Result<(T, Span)> {
    if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;
        let value: T = input.parse()?;
        let span = value.span();
        Ok((value, span))
    } else if input.peek(rust_syntax::token::Paren) {
        let content;
        let parentheses = parenthesized!(content in input);
        let value = content.parse()?;
        // rust-syntax tracks leftover nested tokens until the outer parse
        // completes. This rejects trailing junk while preserving its original
        // delimiter diagnostic rather than decorating it as a value error.
        Ok((value, parentheses.span.join()))
    } else {
        Err(input.error("expected `=` or `(`"))
    }
}

fn insert(slot: &mut Option<Field>, field: Field, name: &str) -> Result<()> {
    if let Some(first) = slot {
        let message = format!("`{name}` is specified multiple times");
        return Err(paired_error(
            first.value_span,
            field.value_span,
            message.clone(),
            message,
        ));
    }
    *slot = Some(field);
    Ok(())
}

fn paired_error(first: Span, second: Span, forward: String, reverse: String) -> Error {
    if let Some(span) = first.join(second) {
        Error::new(span, forward)
    } else {
        let mut error = Error::new(first, forward);
        error.combine(Error::new(second, reverse));
        error
    }
}

#[cfg(test)]
#[path = "../../../test/view-macros/unit/prop_options.rs"]
mod tests;
