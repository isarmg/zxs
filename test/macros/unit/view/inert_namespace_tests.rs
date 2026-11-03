use super::*;

fn expansion(input: TokenStream) -> String {
    let mut nodes = rstml::parse2(input).unwrap();
    render_view(&mut nodes, None, None, false)
        .unwrap()
        .unwrap()
        .to_string()
}

#[test]
fn ambiguous_static_children_use_the_parent_svg_namespace() {
    let svg_inert = quote! { ::web_interface::view_renderer::svg::InertElement::new }.to_string();
    let html_inert = quote! { ::web_interface::view_renderer::html::InertElement::new }.to_string();
    for input in [
        quote! { <svg><title>"label"</title></svg> },
        quote! { <svg><a href="#target">"link"</a></svg> },
        quote! { <svg><title>"label"</title><circle /></svg> },
    ] {
        let actual = expansion(input.clone());
        assert!(actual.contains(&svg_inert), "{input}: {actual}");
        assert!(!actual.contains(&html_inert), "{input}: {actual}");
    }
}

#[test]
fn html_ambiguous_children_and_complete_svg_roots_keep_html_parsing() {
    let html_inert = quote! { ::web_interface::view_renderer::html::InertElement::new }.to_string();
    let svg_inert = quote! { ::web_interface::view_renderer::svg::InertElement::new }.to_string();
    for input in [
        quote! { <div><title>"label"</title></div> },
        quote! { <div><a href="#target">"link"</a></div> },
        quote! { <div><svg id="nested"><title>"label"</title></svg></div> },
    ] {
        let actual = expansion(input.clone());
        assert!(actual.contains(&html_inert), "{input}: {actual}");
        assert!(!actual.contains(&svg_inert), "{input}: {actual}");
    }
}

#[test]
fn unambiguous_svg_static_children_keep_svg_parsing() {
    let actual = expansion(quote! { <svg><g id="group"><title>"label"</title></g></svg> });
    let expected = quote! { ::web_interface::view_renderer::svg::InertElement::new }.to_string();
    assert!(actual.contains(&expected), "{actual}");
}
