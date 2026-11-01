extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use rust_syntax::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Token,
};

struct SliceMacroInput {
    root: rust_syntax::Ident,
    path: Punctuated<rust_syntax::Member, Token![.]>,
}

impl Parse for SliceMacroInput {
    fn parse(input: ParseStream) -> rust_syntax::Result<Self> {
        let root: rust_syntax::Ident = input.parse()?;
        input.parse::<Token![.]>()?;
        // do not accept trailing punctuation
        let path: Punctuated<rust_syntax::Member, Token![.]> =
            Punctuated::parse_separated_nonempty(input)?;

        if !input.is_empty() {
            return Err(input.error("unexpected token"));
        }

        Ok(Self { root, path })
    }
}

impl ToTokens for SliceMacroInput {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let root = &self.root;
        let path = &self.path;

        tokens.extend(quote! {
            ::web_interface::reactive::computed::create_slice(
                #root,
                |st: &_| st.#path.clone(),
                |st: &mut _, n| st.#path = n
            )
        })
    }
}

pub fn slice_impl(tokens: TokenStream) -> TokenStream {
    let input = parse_macro_input!(tokens as SliceMacroInput);
    input.into_token_stream().into()
}

#[cfg(test)]
#[path = "../../../test/macros/unit/slice/tests.rs"]
mod tests;
