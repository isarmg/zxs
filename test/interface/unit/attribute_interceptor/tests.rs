use super::*;

fn exists<T: RenderHtml>(_: &T) -> bool {
    T::EXISTS
}

#[test]
fn empty_children_preserve_node_absence_through_attribute_and_owned_wrappers() {
    assert!(!exists(&()));
    let view = AttributeInterceptorInner::new(|_| ());
    assert!(!exists(&view));

    let view = view.add_any_attr(());
    assert!(!exists(&view));

    let view = view.into_owned();
    assert!(!exists(&view));
}

#[test]
fn present_children_preserve_node_presence_through_attribute_and_owned_wrappers() {
    assert!(exists(&"present"));
    let view = AttributeInterceptorInner::new(|_| "present");
    assert!(exists(&view));

    let view = view.add_any_attr(());
    assert!(exists(&view));

    let view = view.into_owned();
    assert!(exists(&view));
}
