use super::*;

#[test]
fn semantic_attributes_are_not_serialized_as_inert_markup() {
    for attribute in [
        quote! { inner_html="<b>content</b>" },
        quote! { node_ref="not a node reference" },
        quote! { class:active="not a boolean" },
        quote! { on:click },
        quote! { prop:value },
        quote! { style:color },
        quote! { use:decorate },
        quote! { bind:value },
    ] {
        let input = quote! { <div #attribute /> };
        let nodes = rstml::parse2(input.clone()).unwrap();
        assert!(!is_inert_element(&nodes[0]), "{input}");
    }
}

#[test]
fn nested_inner_html_keeps_the_normal_attribute_lowering() {
    for input in [
        quote! { <div><div inner_html="<b>content</b>" /></div> },
        quote! { <svg><g inner_html="<path />" /></svg> },
    ] {
        let mut normal = rstml::parse2(input.clone()).unwrap();
        let expected = render_view(&mut normal, None, None, true).unwrap().unwrap();
        let mut optimized = rstml::parse2(input.clone()).unwrap();
        let actual = render_view(&mut optimized, None, None, false)
            .unwrap()
            .unwrap();
        assert_eq!(actual.to_string(), expected.to_string(), "{input}");
    }
}

#[test]
fn ordinary_html_and_svg_stay_inert_and_escape_literal_content() {
    let nodes = rstml::parse2(quote! {
        <div data-label="<&\"" class="base">"<text>&"<input disabled /></div>
    })
    .unwrap();
    assert!(is_inert_element(&nodes[0]));
    let actual = inert_element_to_tokens(&nodes[0], true, None);
    let expected = quote! {
        ::web_interface::view_renderer::html::InertElement::new(
            "<div data-label=\"&lt;&amp;&quot;\" class=\"base\">&lt;text&gt;&amp;<input disabled></div>"
        )
    };
    assert_eq!(actual.to_string(), expected.to_string());

    let nodes = rstml::parse2(quote! { <g data-label="<&\""><path d="M0 0" /></g> }).unwrap();
    assert!(is_inert_element(&nodes[0]));
    let actual = inert_svg_element_to_tokens(&nodes[0], true, None);
    let expected = quote! {
        ::web_interface::view_renderer::svg::InertElement::new(
            "<g data-label=\"&lt;&amp;&quot;\"><path d=\"M0 0\"></path></g>"
        )
    };
    assert_eq!(actual.to_string(), expected.to_string());
}
