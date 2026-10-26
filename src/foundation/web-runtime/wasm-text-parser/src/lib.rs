//! A crate for low-level parsing of the WebAssembly text formats: WAT and WAST.
//!
//! This local component supplies binding tests with WAT input and directives,
//! providing a low-level parsing API for parsing WebAssembly text format
//! structures. The API provided by this crate is very similar to
//! [`syn`](https://docs.rs/syn) and provides the ability to write customized
//! parsers which may be an extension to the core WebAssembly text format. For
//! more documentation see the [`parser`] module.
//!
//! # High-level Overview
//!
//! This crate provides a few major pieces of functionality
//!
//! * [`lexer`] - this is a raw lexer for the wasm text format. This is not
//!   customizable, but if you'd like to iterate over raw tokens this is the
//!   module for you. You likely won't use this much.
//!
//! * [`parser`] - this is the workhorse of this crate. The [`parser`] module
//!   provides the [`Parse`][] trait primarily and utilities
//!   around working with a [`Parser`](`parser::Parser`) to parse streams of
//!   tokens.
//!
//! * [`Module`](crate::core::Module) - this contains an Abstract Syntax Tree
//!   (AST) of the WebAssembly Text format (WAT) as well as the unofficial WAST
//!   format. This also has a [`Module::encode`](crate::core::Module::encode)
//!   method to emit a module in its binary form.
//!
//! The implementation originates from wast 245.0.1. Current source identity,
//! supported consumers and original material are tracked by repository provenance.
//!
//! Core WebAssembly module parsing is always included.
//!
//! DWARF generation is not included in this local text parser.
//!
//! [`Parse`]: parser::Parse
//! [`LexError`]: lexer::LexError

#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![cfg_attr(docsrs, feature(doc_cfg))]

/// A macro to create a custom keyword parser.
///
/// This macro is invoked in one of two forms:
///
/// ```
/// // keyword derived from the Rust identifier:
/// wasm_text_parser::custom_keyword!(foo);
///
/// // or an explicitly specified string representation of the keyword:
/// wasm_text_parser::custom_keyword!(my_keyword = "the-wasm-keyword");
/// ```
///
/// This can then be used to parse custom keyword for custom items, such as:
///
/// ```
/// use wasm_text_parser::parser::{Parser, Result, Parse};
///
/// struct InlineModule<'a> {
///     inline_text: &'a str,
/// }
///
/// mod kw {
///     wasm_text_parser::custom_keyword!(inline);
/// }
///
/// // Parse an inline string module of the form:
/// //
/// //    (inline "(module (func))")
/// impl<'a> Parse<'a> for InlineModule<'a> {
///     fn parse(parser: Parser<'a>) -> Result<Self> {
///         parser.parse::<kw::inline>()?;
///         Ok(InlineModule {
///             inline_text: parser.parse()?,
///         })
///     }
/// }
/// ```
///
/// Note that the keyword name can only start with a lower-case letter, i.e. 'a'..'z'.
#[macro_export]
macro_rules! custom_keyword {
    ($name:ident) => {
        $crate::custom_keyword!($name = stringify!($name));
    };
    ($name:ident = $kw:expr) => {
        #[allow(non_camel_case_types)]
        #[allow(missing_docs)]
        #[derive(Debug, Copy, Clone)]
        pub struct $name(#[allow(dead_code)] pub $crate::token::Span);

        impl<'a> $crate::parser::Parse<'a> for $name {
            fn parse(parser: $crate::parser::Parser<'a>) -> $crate::parser::Result<Self> {
                parser.step(|c| {
                    if let Some((kw, rest)) = c.keyword()? {
                        if kw == $kw {
                            return Ok(($name(c.cur_span()), rest));
                        }
                    }
                    Err(c.error(concat!("expected keyword `", $kw, "`")))
                })
            }
        }

        impl $crate::parser::Peek for $name {
            fn peek(cursor: $crate::parser::Cursor<'_>) -> $crate::parser::Result<bool> {
                Ok(if let Some((kw, _rest)) = cursor.keyword()? {
                    kw == $kw
                } else {
                    false
                })
            }

            fn display() -> &'static str {
                concat!("`", $kw, "`")
            }
        }
    };
}

/// A macro for defining custom reserved symbols.
///
/// This is like `custom_keyword!` but for reserved symbols (`Token::Reserved`)
/// instead of keywords (`Token::Keyword`).
///
///
/// ```
/// # mod example {
/// use wasm_text_parser::parser::{Parser, Result, Parse};
///
/// // Define a custom reserved symbol, the "spaceship" operator: `<=>`.
/// wasm_text_parser::custom_reserved!(spaceship = "<=>");
///
/// /// A "three-way comparison" like `(<=> a b)` that returns -1 if `a` is less
/// /// than `b`, 0 if they're equal, and 1 if `a` is greater than `b`.
/// struct ThreeWayComparison<'a> {
///     lhs: wasm_text_parser::core::Expression<'a>,
///     rhs: wasm_text_parser::core::Expression<'a>,
/// }
///
/// impl<'a> Parse<'a> for ThreeWayComparison<'a> {
///     fn parse(parser: Parser<'a>) -> Result<Self> {
///         parser.parse::<spaceship>()?;
///         let lhs = parser.parse()?;
///         let rhs = parser.parse()?;
///         Ok(ThreeWayComparison { lhs, rhs })
///     }
/// }
/// # }
/// ```
#[macro_export]
macro_rules! custom_reserved {
    ($name:ident) => {
        $crate::custom_reserved!($name = stringify!($name));
    };
    ($name:ident = $rsv:expr) => {
        #[allow(non_camel_case_types)]
        #[allow(missing_docs)]
        #[derive(Debug)]
        pub struct $name(pub $crate::token::Span);

        impl<'a> $crate::parser::Parse<'a> for $name {
            fn parse(parser: $crate::parser::Parser<'a>) -> $crate::parser::Result<Self> {
                parser.step(|c| {
                    if let Some((rsv, rest)) = c.reserved()? {
                        if rsv == $rsv {
                            return Ok(($name(c.cur_span()), rest));
                        }
                    }
                    Err(c.error(concat!("expected reserved symbol `", $rsv, "`")))
                })
            }
        }

        impl $crate::parser::Peek for $name {
            fn peek(cursor: $crate::parser::Cursor<'_>) -> Result<bool> {
                if let Some((rsv, _rest)) = cursor.reserved()? {
                    Ok(rsv == $rsv)
                } else {
                    Ok(false)
                }
            }

            fn display() -> &'static str {
                concat!("`", $rsv, "`")
            }
        }
    };
}

/// A macro, like [`custom_keyword`], to create a type which can be used to
/// parse/peek annotation directives.
///
/// Note that when you're parsing custom annotations it can be somewhat tricky
/// due to the nature that most of them are skipped. You'll want to be sure to
/// consult the documentation of [`Parser::register_annotation`][register] when
/// using this macro.
///
/// # Examples
///
/// To see an example of how to use this macro, let's invent our own syntax for
/// the [producers section][section] which looks like:
///
/// ```wat
/// (@producer "wat" "1.0.2")
/// ```
///
/// Here, for simplicity, we'll assume everything is a `processed-by` directive,
/// but you could get much more fancy with this as well.
///
/// ```
/// # use wasm_text_parser::*;
/// # use wasm_text_parser::parser::*;
///
/// // First we define the custom annotation keyword we're using, and by
/// // convention we define it in an `annotation` module.
/// mod annotation {
///     wasm_text_parser::annotation!(producer);
/// }
///
/// struct Producer<'a> {
///     name: &'a str,
///     version: &'a str,
/// }
///
/// impl<'a> Parse<'a> for Producer<'a> {
///     fn parse(parser: Parser<'a>) -> Result<Self> {
///         // Remember that parser conventionally parse the *interior* of an
///         // s-expression, so we parse our `@producer` annotation and then we
///         // parse the payload of our annotation.
///         parser.parse::<annotation::producer>()?;
///         Ok(Producer {
///             name: parser.parse()?,
///             version: parser.parse()?,
///         })
///     }
/// }
/// ```
///
/// Note though that this is only half of the parser for annotations. The other
/// half is calling the [`register_annotation`][register] method at the right
/// time to ensure the parser doesn't automatically skip our `@producer`
/// directive. Note that we *can't* call it inside the `Parse for Producer`
/// definition because that's too late and the annotation would already have
/// been skipped.
///
/// Instead we'll need to call it from a higher level-parser before the
/// parenthesis have been parsed, like so:
///
/// ```
/// # use wasm_text_parser::*;
/// # use wasm_text_parser::parser::*;
/// struct Module<'a> {
///     fields: Vec<ModuleField<'a>>,
/// }
///
/// impl<'a> Parse<'a> for Module<'a> {
///     fn parse(parser: Parser<'a>) -> Result<Self> {
///         // .. parse module header here ...
///
///         // register our custom `@producer` annotation before we start
///         // parsing the parentheses of each field
///         let _r = parser.register_annotation("producer");
///
///         let mut fields = Vec::new();
///         while !parser.is_empty() {
///             fields.push(parser.parens(|p| p.parse())?);
///         }
///         Ok(Module { fields })
///     }
/// }
///
/// enum ModuleField<'a> {
///     Producer(Producer<'a>),
///     // ...
/// }
/// # struct Producer<'a>(&'a str);
/// # impl<'a> Parse<'a> for Producer<'a> {
/// #   fn parse(parser: Parser<'a>) -> Result<Self> { Ok(Producer(parser.parse()?)) }
/// # }
/// # mod annotation { wasm_text_parser::annotation!(producer); }
///
/// impl<'a> Parse<'a> for ModuleField<'a> {
///     fn parse(parser: Parser<'a>) -> Result<Self> {
///         // and here `peek` works and our delegated parsing works because the
///         // annotation has been registered.
///         if parser.peek::<annotation::producer>()? {
///             return Ok(ModuleField::Producer(parser.parse()?));
///         }
///
///         // .. typically we'd parse other module fields here...
///
///         Err(parser.error("unknown module field"))
///     }
/// }
/// ```
///
/// [register]: crate::parser::Parser::register_annotation
/// [section]: https://github.com/WebAssembly/tool-conventions/blob/master/ProducersSection.md
#[macro_export]
macro_rules! annotation {
    ($name:ident) => {
        $crate::annotation!($name = stringify!($name));
    };
    ($name:ident = $annotation:expr) => {
        #[allow(non_camel_case_types)]
        #[allow(missing_docs)]
        #[derive(Debug)]
        pub struct $name(pub $crate::token::Span);

        impl<'a> $crate::parser::Parse<'a> for $name {
            fn parse(parser: $crate::parser::Parser<'a>) -> $crate::parser::Result<Self> {
                parser.step(|c| {
                    if let Some((a, rest)) = c.annotation()? {
                        if a == $annotation {
                            return Ok(($name(c.cur_span()), rest));
                        }
                    }
                    Err(c.error(concat!("expected annotation `@", $annotation, "`")))
                })
            }
        }

        impl $crate::parser::Peek for $name {
            fn peek(cursor: $crate::parser::Cursor<'_>) -> $crate::parser::Result<bool> {
                Ok(if let Some((a, _rest)) = cursor.annotation()? {
                    a == $annotation
                } else {
                    false
                })
            }

            fn display() -> &'static str {
                concat!("`@", $annotation, "`")
            }
        }
    };
}

pub mod lexer;
pub mod parser;
pub mod token;

mod encode;
mod error;

mod gensym;

mod names;
pub use self::error::*;

mod wat;
pub use self::wat::*;

// Support for core wasm parsing
pub mod core;

/// Common keyword used to parse WebAssembly text files.
pub mod kw {
    custom_keyword!(after);
    custom_keyword!(any);
    custom_keyword!(array);
    custom_keyword!(before);
    custom_keyword!(binary);
    custom_keyword!(catch);
    custom_keyword!(catch_ref);
    custom_keyword!(catch_all);
    custom_keyword!(catch_all_ref);
    custom_keyword!(code);
    custom_keyword!(cont);
    custom_keyword!(component);
    custom_keyword!(data);
    custom_keyword!(declare);
    custom_keyword!(descriptor);
    custom_keyword!(describes);
    custom_keyword!(elem);
    custom_keyword!(exact);
    custom_keyword!(tag);
    custom_keyword!(exn);
    custom_keyword!(export);
    custom_keyword!(r#extern = "extern");
    custom_keyword!(eq);
    custom_keyword!(f32);
    custom_keyword!(f32x4);
    custom_keyword!(f64);
    custom_keyword!(f64x2);
    custom_keyword!(field);
    custom_keyword!(first);
    custom_keyword!(func);
    custom_keyword!(global);
    custom_keyword!(i16);
    custom_keyword!(i16x8);
    custom_keyword!(i31);
    custom_keyword!(i32);
    custom_keyword!(i32x4);
    custom_keyword!(i64);
    custom_keyword!(i64x2);
    custom_keyword!(i8);
    custom_keyword!(i8x16);
    custom_keyword!(import);
    custom_keyword!(item);
    custom_keyword!(last);
    custom_keyword!(local);
    custom_keyword!(memory);
    custom_keyword!(module);
    custom_keyword!(nocont);
    custom_keyword!(nofunc);
    custom_keyword!(noextern);
    custom_keyword!(noexn);
    custom_keyword!(none);
    custom_keyword!(null);
    custom_keyword!(offset);
    custom_keyword!(on);
    custom_keyword!(pagesize);
    custom_keyword!(param);
    custom_keyword!(r#else = "else");
    custom_keyword!(r#mut = "mut");
    custom_keyword!(r#type = "type");
    custom_keyword!(r#ref = "ref");
    custom_keyword!(rec);
    custom_keyword!(acq_rel);
    custom_keyword!(result);
    custom_keyword!(seq_cst);
    custom_keyword!(shared);
    custom_keyword!(start);
    custom_keyword!(sub);
    custom_keyword!(switch);
    custom_keyword!(r#final = "final");
    custom_keyword!(table);
    custom_keyword!(then);
    custom_keyword!(v128);
    custom_keyword!(r#struct = "struct");
    custom_keyword!(language);
    custom_keyword!(sdk);
    custom_keyword!(processed_by = "processed-by");
    custom_keyword!(mem_info = "mem-info");
    custom_keyword!(needed);
    custom_keyword!(export_info = "export-info");
    custom_keyword!(import_info = "import-info");
    custom_keyword!(runtime_path = "runtime-path");
}

/// Common annotations used to parse WebAssembly text files.
pub mod annotation {
    annotation!(custom);
    annotation!(name);
    annotation!(producers);
    annotation!(dylink_0 = "dylink.0");
    annotation!(metadata_code_branch_hint = "metadata.code.branch_hint");
}
