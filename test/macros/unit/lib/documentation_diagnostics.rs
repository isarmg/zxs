use quote::quote;

#[test]
fn event_on_slot_reports_the_unsupported_target() {
    let mut nodes = rstml::parse2(quote! {
        <ComponentWithSlot>
            <SlotWithChildren slot:slot on:click=move |_| {}>
                <h1>"Hello, World!"</h1>
            </SlotWithChildren>
        </ComponentWithSlot>
    })
    .unwrap();
    let error = super::view::render_view(&mut nodes, None, None, false).unwrap_err();
    assert_eq!(
            error.to_string(),
            "event handlers cannot be attached to slots; attach the handler to an element inside the slot instead"
        );
}

#[test]
fn sibling_context_does_not_admit_slots_without_a_component() {
    for input in [
        quote! { <div><Slot slot /></div> },
        quote! { <div><Slot slot /><span /></div> },
        quote! { <div><span /><Slot slot /></div> },
        quote! { <><Slot slot /><span /></> },
    ] {
        let mut nodes = rstml::parse2(input.clone()).unwrap();
        let error = super::view::render_view(&mut nodes, None, None, true)
            .expect_err("a sibling must not provide a component slot context");
        assert_eq!(
            error.to_string(),
            "slots cannot be used inside HTML elements",
            "{input}"
        );
    }
}
