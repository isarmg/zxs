use super::*;

#[test]
fn invalid_prop_names_reach_expansion_as_source_diagnostics() {
    for pattern in ["value", "(value,)"] {
        let source = format!("fn Example(#[prop(name = \"not-a-name\")] {pattern}: (u8,)) {{}}");
        let model = rust_syntax::parse_str::<Model>(&source).unwrap();
        let error = model.expand(&mut Errors::default()).unwrap_err();
        assert_eq!(
            error.span().source_text().as_deref(),
            Some("\"not-a-name\"")
        );
    }
}

#[test]
fn renamed_props_keep_raw_identifier_tokens_and_source() {
    for pattern in ["value", "(value,)"] {
        let source = format!("fn Example(#[prop(name = \"r#type\")] {pattern}: (u8,)) {{}}");
        let model = rust_syntax::parse_str::<Model>(&source).unwrap();
        assert_eq!(model.props[0].name.ident.to_string(), "r#type");
        assert_eq!(
            model.props[0].name.ident.span().source_text().as_deref(),
            Some("\"r#type\"")
        );
        let expanded = model.expand(&mut Errors::default()).unwrap();
        rust_syntax::parse2::<rust_syntax::File>(expanded).unwrap();
    }
}

#[test]
fn binding_modifiers_stay_in_the_body_not_the_props_fields() {
    for pattern in ["value", "mut value", "ref value", "ref mut value"] {
        let source = format!("fn Example({pattern}: String) {{}}");
        let model = rust_syntax::parse_str::<Model>(&source).unwrap();
        assert_eq!(
            model.body.sig.inputs.to_token_stream().to_string(),
            format!("{pattern} : String")
        );
        let expanded = model.expand(&mut Errors::default()).unwrap();
        let file = rust_syntax::parse2::<rust_syntax::File>(expanded).unwrap();
        let Item::Struct(props) = &file.items[0] else {
            panic!("expected generated props struct");
        };
        let field = props.fields.iter().next().unwrap();
        assert_eq!(field.ident.as_ref().unwrap(), "value");
        assert_eq!(field.ty.to_token_stream().to_string(), "String");
    }
}
