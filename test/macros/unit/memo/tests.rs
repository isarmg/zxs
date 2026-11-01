use super::*;

#[test]
fn empty_and_malformed_paths_are_rejected() {
    for source in [
        "",
        "state",
        "state.",
        "state..field",
        "state.field.",
        "state.field extra",
        "state.field,",
        "state.field()",
    ] {
        assert!(
            rust_syntax::parse_str::<MemoMacroInput>(source).is_err(),
            "accepted {source:?}"
        );
    }
}

#[test]
fn named_and_tuple_paths_preserve_the_generated_access() {
    for source in [
        "state.field",
        "state.inner.field",
        "state.0.field",
        "state.field.1",
    ] {
        let input = rust_syntax::parse_str::<MemoMacroInput>(source).unwrap();
        assert_eq!(input.root.to_string(), "state");
        let expected: proc_macro2::TokenStream =
            source.strip_prefix("state.").unwrap().parse().unwrap();
        assert_eq!(
            input.path.to_token_stream().to_string(),
            expected.to_string()
        );
        let root = &input.root;
        let path = &input.path;
        let expected = quote! {
            ::web_interface::reactive::computed::Memo::new(
                move |_| {
                    use ::web_interface::reactive::traits::With;
                    #root.with(|st: _| st.#path.clone())
                }
            )
        };
        assert_eq!(input.to_token_stream().to_string(), expected.to_string());
    }
}
