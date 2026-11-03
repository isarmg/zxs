use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use view_renderer::{
    html::{
        element::{button, div, input, span},
        event,
    },
    prelude::*,
    view::{template::ViewTemplate, ToTemplate},
};
use wasm_bindgen::{prelude::*, JsCast};

fn template(label: String, counter: Arc<AtomicUsize>) -> impl RenderHtml + ToTemplate {
    let active = label.starts_with("初始");
    div()
        .id("csr-template-probe")
        .class("template-probe")
        .class(("active", active))
        .style(("color", "rgb(1, 2, 3)"))
        .child((
            input().prop("value", label.clone()),
            button()
                .on(event::click, move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .child(label),
            span().id("primitive-probe").child((42u32, '<', true)),
        ))
}

fn boolean_template(value: bool) -> impl RenderHtml + ToTemplate {
    input()
        .r#type("checkbox")
        .disabled(value)
        .checked(value)
        .required(value)
}

async fn settle_properties() {
    let _ =
        js_sys::futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED)).await;
}

fn inert_namespace_cache_contract(parent: &web_sys::Element) -> Result<(), JsValue> {
    for (markup, order, label) in [
        (
            "<a data-zxs-namespace-order=\"html-first\">html-first</a>",
            [false, true, false, true],
            "html-first",
        ),
        (
            "<a data-zxs-namespace-order=\"svg-first\">svg-first</a>",
            [true, false, true, false],
            "svg-first",
        ),
    ] {
        let mut previous: Vec<web_sys::Element> = Vec::new();
        for svg in order {
            let node = if svg {
                let mut state = view_renderer::svg::InertElement::new(markup).build();
                state.mount(parent, None);
                let node = state.elements().into_iter().next().unwrap();
                state.unmount();
                node
            } else {
                let mut state = view_renderer::html::InertElement::new(markup).build();
                state.mount(parent, None);
                let node = state.elements().into_iter().next().unwrap();
                state.unmount();
                node
            };
            let (namespace, tag) = if svg {
                ("http://www.w3.org/2000/svg", "a")
            } else {
                ("http://www.w3.org/1999/xhtml", "A")
            };
            if node.namespace_uri().as_deref() != Some(namespace)
                || node.tag_name() != tag
                || node.get_attribute("data-zxs-namespace-order").as_deref() != Some(label)
                || node.text_content().as_deref() != Some(label)
            {
                return Err(
                    format!("inert cache namespace/root/content: {label}, svg={svg}").into(),
                );
            }
            if previous.iter().any(|old| old.is_same_node(Some(&node))) {
                return Err(format!("inert cache did not clone independently: {label}").into());
            }
            previous.push(node);
        }
    }
    Ok(())
}

#[wasm_bindgen]
pub async fn run_template_probe() -> Result<String, JsValue> {
    let owner = reactive_state::owner::Owner::new();
    owner.with(|| reactive_state::computed::ScopedFuture::new(async {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut state = ViewTemplate::new(template("初始 <&> Ω".into(), counter.clone())).build();
        let document = web_sys::window().unwrap().document().unwrap();
        let parent = document.body().unwrap();
        // The typed API closure must not become a markup or custom-event whitelist.
        let custom_counter = counter.clone();
        let mut custom = view_renderer::html::element::custom("zxs-scope-probe")
            .add_any_attr(view_renderer::html::attribute::custom::custom_attribute("data-文件", "Ω"))
            .on(event::Custom::<web_sys::Event>::new("zxs-scope-event"), move |_| {
                custom_counter.fetch_add(1, Ordering::SeqCst);
            }).build();
        custom.mount(parent.as_ref(), None);
        let custom_node = custom.elements().into_iter().next().unwrap();
        if custom_node.tag_name() != "ZXS-SCOPE-PROBE" || custom_node.get_attribute("data-文件").as_deref() != Some("Ω") {
            return Err("custom element and Unicode attribute".into());
        }
        custom_node.dispatch_event(&web_sys::Event::new("zxs-scope-event")?)?;
        if counter.load(Ordering::SeqCst) != 1 { return Err("custom event".into()); }
        custom.unmount();
        drop(custom);
        custom_node.dispatch_event(&web_sys::Event::new("zxs-scope-event")?)?;
        if counter.load(Ordering::SeqCst) != 1 { return Err("custom event cleanup".into()); }
        counter.store(0, Ordering::SeqCst);
        let mut svg = view_renderer::svg::InertElement::new("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0L1 1\"/></svg>").build();
        svg.mount(parent.as_ref(), None);
        let svg_node = svg.elements().into_iter().next().unwrap();
        if svg_node.namespace_uri().as_deref() != Some("http://www.w3.org/2000/svg") || svg_node.query_selector("path")?.is_none() {
            return Err("inert SVG namespace".into());
        }
        svg.unmount();
        inert_namespace_cache_contract(parent.as_ref())?;
        state.mount(parent.as_ref(), None);
        let root = state.elements().into_iter().next().unwrap();
        let mut flags = ViewTemplate::new(boolean_template(false)).build();
        flags.mount(parent.as_ref(), None);
        let flag_node = flags.elements().into_iter().next().unwrap();
        for key in ["disabled", "checked", "required"] {
            if flag_node.has_attribute(key) { return Err(format!("boolean initial false: {key}").into()); }
        }
        for value in [false, true, false] {
            ViewTemplate::new(boolean_template(value)).rebuild(&mut flags);
            for key in ["disabled", "checked", "required"] {
                if flag_node.has_attribute(key) != value { return Err(format!("boolean rebuild {value}: {key}").into()); }
            }
        }
        flags.unmount();
        drop(flags);
        let button = root.query_selector("button")?.unwrap();
        let input: web_sys::HtmlInputElement = root.query_selector("input")?.unwrap().unchecked_into();
        if root.id() != "csr-template-probe" || !root.class_list().contains("template-probe") || !root.class_list().contains("active") {
            return Err("template attributes".into());
        }
        settle_properties().await;
        if button.text_content().as_deref() != Some("初始 <&> Ω") || input.value() != "初始 <&> Ω" {
            return Err(format!("template dynamic text and property: text={:?}, input={:?}, html={}", button.text_content(), input.value(), root.outer_html()).into());
        }
        if root.query_selector("#primitive-probe")?.unwrap().text_content().as_deref() != Some("42<true") {
            return Err("primitive template text and escaping".into());
        }
        if !root.get_attribute("style").unwrap_or_default().contains("rgb(1, 2, 3)") {
            return Err("template style".into());
        }
        if button.children().length() != 0 { return Err("text was interpreted as HTML".into()); }
        button.dispatch_event(&web_sys::Event::new("click")?)?;
        if counter.load(Ordering::SeqCst) != 1 { return Err("template event listener".into()); }
        input.focus()?;
        ViewTemplate::new(template("更新 文件 🌍".into(), counter.clone())).rebuild(&mut state);
        settle_properties().await;
        let rebuilt = state.elements().into_iter().next().unwrap();
        if !root.is_same_node(Some(&rebuilt)) || input.value() != "更新 文件 🌍" || button.text_content().as_deref() != Some("更新 文件 🌍") {
            return Err("template rebuild and node retention".into());
        }
        if root.class_list().contains("active") { return Err("template class toggle rebuild".into()); }
        if !input.is_same_node(document.active_element().as_ref().map(|el| el.as_ref())) {
            return Err("template rebuild focus retention".into());
        }
        button.dispatch_event(&web_sys::Event::new("click")?)?;
        if counter.load(Ordering::SeqCst) != 2 { return Err("template listener replaced once".into()); }
        state.unmount();
        drop(state);
        if document.get_element_by_id("csr-template-probe").is_some() { return Err("template unmount".into()); }
        button.dispatch_event(&web_sys::Event::new("click")?)?;
        if counter.load(Ordering::SeqCst) != 2 { return Err("template handler cleanup".into()); }
        Ok("{\"status\":\"passed\",\"checks\":[\"custom element and Unicode attribute\",\"custom event and cleanup\",\"inert SVG namespace\",\"inert cache HTML then SVG\",\"inert cache SVG then HTML\",\"inert cache repeated clones\",\"boolean initial false\",\"boolean false→false\",\"boolean false→true\",\"boolean true→false\",\"template attributes\",\"dynamic text\",\"DOM property\",\"primitive text and escaping\",\"style\",\"class toggle rebuild\",\"text escaping\",\"event listener\",\"rebuild\",\"node retention\",\"focus retention\",\"listener replacement\",\"unmount and cleanup\"]}".into())
    })).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn html<V: ToTemplate>(_: &V) -> String {
        let mut value = String::new();
        V::to_template(
            &mut value,
            &mut String::new(),
            &mut String::new(),
            &mut String::new(),
            &mut Default::default(),
        );
        value
    }
    #[test]
    fn void_template_nodes_do_not_create_sibling_placeholders() {
        let value = template("value".into(), Arc::new(AtomicUsize::new(0)));
        let output = html(&value);
        assert!(output.contains("<input><button>"), "{output}");
        assert!(!output.contains("</input>"), "{output}");
        fn constant<V: ToTemplate>(_: &V) -> &'static str {
            V::TEMPLATE
        }
        assert!(!constant(&value).contains("</input>"));
    }
}
