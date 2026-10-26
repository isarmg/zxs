use crate::core::{Module, ModuleField, ModuleKind};
use crate::kw;
use crate::parser::{Parse, Parser, Result};
use crate::token::Span;

/// A `*.wat` file parser, or a parser for one parenthesized module.
///
/// This is the top-level type which you'll frequently parse when working with
/// this crate. A `*.wat` file is either one `module` s-expression or a sequence
/// of s-expressions that are module fields.
#[derive(Debug)]
pub struct Wat<'a>(Module<'a>);

impl Wat<'_> {
    /// Encodes this core module to binary form.
    pub fn encode(&mut self) -> std::result::Result<Vec<u8>, crate::Error> {
        self.0.encode()
    }
}

impl<'a> Parse<'a> for Wat<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if !parser.has_meaningful_tokens() {
            return Err(parser.error("expected at least one module field"));
        }

        parser.with_standard_annotations_registered(|parser| {
            let wat = if parser.peek2::<kw::module>()? {
                Wat(parser.parens(|parser| parser.parse())?)
            } else if parser.peek2::<kw::component>()? {
                return Err(parser.error("component text is outside the core parser scope"));
            } else {
                let fields = ModuleField::parse_remaining(parser)?;
                Wat(Module {
                    span: Span { offset: 0 },
                    id: None,
                    name: None,
                    kind: ModuleKind::Text(fields),
                })
            };
            Ok(wat)
        })
    }
}
