use super::*;

#[test]
fn exact_modifiers_work_in_any_order() {
    for event in [
        "click:target:capture:undelegated",
        "click:undelegated:target:capture",
    ] {
        let (name, options) = parse_event(event);
        assert_eq!(name, "click");
        assert!(options.targeted && options.captured && options.undelegated);
        assert!(!parse_event_name(event).1);
    }
}

#[test]
fn custom_event_segments_are_not_partial_modifiers() {
    for event in [
        "dialog:targetChanged",
        "dialog:capturecomplete",
        "dialog:undelegateddone",
    ] {
        let (name, options) = parse_event(event);
        assert_eq!(name, event);
        assert!(!options.targeted && !options.captured && !options.undelegated);
        let (tokens, custom, _) = parse_event_name(event);
        assert!(custom);
        assert_eq!(
            tokens.to_string(),
            quote! { Custom::new(#event) }.to_string()
        );
    }
}

#[test]
fn exact_modifier_does_not_erase_similar_custom_segment() {
    let (name, options) = parse_event("dialog:targetChanged:capture");
    assert_eq!(name, "dialog:targetChanged");
    assert!(options.captured);
    assert!(!options.targeted);
}

#[test]
fn bare_custom_event_named_like_modifier_is_preserved() {
    let (name, options) = parse_event("capture");
    assert_eq!(name, "capture");
    assert!(!options.captured);
}

#[test]
fn every_builtin_event_has_valid_rust_tokens() {
    for name in TYPED_EVENTS {
        let (tokens, custom, _) = parse_event_name(name);
        assert!(!custom, "{name}");
        assert_eq!(tokens.to_string(), name);
    }
}

#[test]
fn absolute_attributes_preserve_hyphenated_custom_events() {
    let mut nodes = rstml::parse2(quote! {
        <{..} on:custom-event=handler />
    })
    .unwrap();
    let actual = render_view(&mut nodes, None, None, true).unwrap().unwrap();
    let expected = quote! {
        (::web_interface::view_renderer::html::event::on(
            ::web_interface::view_renderer::html::event::Custom::new("custom-event"),
            handler
        ),)
    };
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn absolute_attributes_keep_custom_event_modifiers_and_data_attributes() {
    let mut nodes = rstml::parse2(quote! {
        <{..} on:custom-event:capture=handler data-event="custom-event" />
    })
    .unwrap();
    let actual = render_view(&mut nodes, None, None, true).unwrap().unwrap();
    let expected = quote! {
        (
            ::web_interface::view_renderer::html::event::on(
                ::web_interface::view_renderer::html::event::capture(
                    ::web_interface::view_renderer::html::event::Custom::new("custom-event")
                ),
                handler
            ),
            ::web_interface::view_renderer::html::attribute::custom::custom_attribute(
                "data-event", "custom-event"
            ),
        )
    };
    assert_eq!(actual.to_string(), expected.to_string());
}
