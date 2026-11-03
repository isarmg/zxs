use core::num::NonZeroUsize;
use web_interface::prelude::*;

#[test]
fn include_view_resolves_its_file_from_the_consumer_crate() {
    let _view = include_view!("../../test/interface/integration/fixtures/included_view.rsx");
}

#[derive(PartialEq, Debug)]
struct UserInfo {
    user_id: String,
    email: String,
}

#[derive(PartialEq, Debug)]
struct Admin(bool);

#[component]
fn Component(
    #[prop(optional)] optional: bool,
    #[prop(optional, into)] optional_into: Option<String>,
    #[prop(optional_no_strip)] optional_no_strip: Option<String>,
    #[prop(strip_option)] strip_option: Option<u8>,
    #[prop(default = NonZeroUsize::new(10).unwrap())] default: NonZeroUsize,
    #[prop(into)] into: String,
    impl_trait: impl Fn() -> i32 + 'static,
    #[prop(name = "data")] UserInfo { email, user_id }: UserInfo,
    #[prop(name = "tuple")] (name, id): (String, i32),
    #[prop(name = "tuple_struct")] Admin(is_admin): Admin,
    #[prop(name = "outside_name")] inside_name: i32,
) -> impl IntoView {
    _ = optional;
    _ = optional_into;
    _ = optional_no_strip;
    _ = strip_option;
    _ = default;
    _ = into;
    _ = impl_trait;
    _ = email;
    _ = user_id;
    _ = id;
    _ = name;
    _ = is_admin;
    _ = inside_name;
}

#[test]
fn component() {
    let cp = ComponentProps::builder()
        .into("")
        .strip_option(9)
        .impl_trait(|| 42)
        .data(UserInfo {
            email: "em@il".into(),
            user_id: "1".into(),
        })
        .tuple(("Joe".into(), 12))
        .tuple_struct(Admin(true))
        .outside_name(1)
        .build();
    assert!(!cp.optional);
    assert_eq!(cp.optional_into, None);
    assert_eq!(cp.optional_no_strip, None);
    assert_eq!(cp.strip_option, Some(9));
    assert_eq!(cp.default, NonZeroUsize::new(10).unwrap());
    assert_eq!(cp.into, "");
    assert_eq!((cp.impl_trait)(), 42);
    assert_eq!(
        cp.data,
        UserInfo {
            email: "em@il".into(),
            user_id: "1".into(),
        }
    );
    assert_eq!(cp.tuple, ("Joe".into(), 12));
    assert_eq!(cp.tuple_struct, Admin(true));
    assert_eq!(cp.outside_name, 1);
}

#[test]
fn component_nostrip() {
    // Should compile (using nostrip:optional_into in second <Component />)
    view! {
        <Component
            optional_into="foo"
            strip_option=9
            into=""
            impl_trait=|| 42
            data=UserInfo {
                email: "em@il".into(),
                user_id: "1".into(),
            }
            tuple=("Joe".into(), 12)
            tuple_struct=Admin(true)
            outside_name=1
        />
        <Component
            nostrip:optional_into=Some("foo")
            strip_option=9
            into=""
            impl_trait=|| 42
            data=UserInfo {
                email: "em@il".into(),
                user_id: "1".into(),
            }
            tuple=("Joe".into(), 12)
            tuple_struct=Admin(true)
            outside_name=1
        />
    };
}

#[component]
fn WithLifetime<'a>(data: &'a str) -> impl IntoView + use<> {
    _ = data;
    "static lifetime"
}

#[test]
fn returns_static_lifetime() {
    #[allow(unused)]
    fn can_return_impl_intoview_from_body() -> impl IntoView {
        let val = String::from("non_static_lifetime");
        WithLifetime(WithLifetimeProps::builder().data(&val).build())
    }
}

#[test]
fn csr_components_preserve_props_children_and_transparent_views() {
    use std::sync::{Arc, Mutex};

    type Calls = Arc<Mutex<Vec<String>>>;

    #[component]
    fn Greeting(#[prop(into)] name: String, calls: Calls, children: Children) -> impl IntoView {
        calls.lock().unwrap().push(name);
        children()
    }

    #[component(transparent)]
    fn PassThrough(calls: Calls, children: Children) -> impl IntoView {
        calls.lock().unwrap().push("transparent".into());
        children()
    }

    let owner = Owner::new();
    let calls = Calls::default();
    owner.with(|| {
        let child_calls = calls.clone();
        let _view = view! {
            <Greeting name="Ada" calls=calls.clone()>
                <PassThrough calls=child_calls.clone()>
                    {child_calls.lock().unwrap().push("children".into()); "welcome"}
                </PassThrough>
            </Greeting>
        };
    });
    assert_eq!(*calls.lock().unwrap(), ["Ada", "transparent", "children"]);
}

#[slot]
struct GenericSlot<'a, T: std::ops::Deref<Target = str> = String, const N: usize = 2>
where
    T: Clone,
{
    value: &'a T::Target,
    labels: [&'a str; N],
}

#[test]
fn slots_preserve_generic_declarations_bounds_and_defaults() {
    let value = String::from("slot");
    let slot: GenericSlot<'_> = GenericSlot::builder()
        .value(value.as_str())
        .labels(["first", "second"])
        .build();
    let slots: Vec<GenericSlot<'_>> = slot.into();
    assert_eq!(slots[0].value, "slot");
    assert_eq!(slots[0].labels, ["first", "second"]);

    let value = "borrowed";
    let slot = GenericSlot::<&str, 1>::builder()
        .value(value)
        .labels(["only"])
        .build();
    assert_eq!(slot.value, "borrowed");
}

mod namespaced_slots {
    use web_interface::prelude::*;

    #[slot]
    pub struct NamedSlot {
        #[prop(into)]
        value: String,
    }
}

#[test]
fn qualified_slot_tags_use_the_struct_name_for_the_default_property() {
    use std::sync::{Arc, Mutex};

    #[component]
    fn SlotConsumer(
        named_slot: namespaced_slots::NamedSlot,
        captured: Arc<Mutex<String>>,
    ) -> impl IntoView {
        *captured.lock().unwrap() = named_slot.value;
    }

    let captured = Arc::new(Mutex::new(String::new()));
    Owner::new().with(|| {
        let _view = view! {
            <SlotConsumer captured=captured.clone()>
                <namespaced_slots::NamedSlot slot value="qualified"/>
            </SlotConsumer>
        };
    });
    assert_eq!(*captured.lock().unwrap(), "qualified");
}

#[slot]
struct TypedSlot<T> {
    #[prop(optional)]
    marker: std::marker::PhantomData<T>,
    #[prop(default = std::any::type_name::<T>())]
    type_name: &'static str,
}

#[test]
fn explicit_slot_generic_arguments_reach_the_builder() {
    use std::sync::{Arc, Mutex};

    #[component]
    fn TypedSlotConsumer<T: Send + Sync + 'static>(
        typed_slot: TypedSlot<T>,
        captured: Arc<Mutex<String>>,
    ) -> impl IntoView {
        *captured.lock().unwrap() = typed_slot.type_name.to_owned();
    }

    let captured = Arc::new(Mutex::new(String::new()));
    Owner::new().with(|| {
        let _view = view! {
            <TypedSlotConsumer captured=captured.clone()>
                <TypedSlot<u8> slot/>
            </TypedSlotConsumer>
        };
    });
    assert_eq!(*captured.lock().unwrap(), "u8");
}

#[test]
fn absolute_attributes_keep_boolean_and_custom_event_semantics() {
    let attributes = view! { <{..} disabled /> };
    let _view = view! { <input {..attributes} /> };

    let attributes = view! {
        <{..} on:custom-event=|_: web_sys::Event| {} />
    };
    let _view = view! { <div {..attributes} /> };

    #[component]
    fn Target() -> impl IntoView {
        view! { <div /> }
    }

    let _view = view! {
        <Target on:custom-event:capture=|_: web_sys::Event| {} />
    };
}

#[test]
fn undelegated_event_descriptors_preserve_capture_identity_and_options() {
    use std::{borrow::Cow, cell::Cell, rc::Rc};
    use web_interface::ev::{EventDescriptor, capture, click, undelegated};

    fn assert_direct<E: EventDescriptor>(_: &E, captured: bool) {
        assert!(!E::BUBBLES);
        assert_eq!(E::CAPTURE, captured);
    }

    let direct = undelegated(click);
    assert_direct(&direct, false);
    assert_eq!(direct.name(), click.name());
    assert_eq!(direct.event_delegation_key(), click.event_delegation_key());
    assert_direct(&undelegated(capture(click)), true);
    assert_direct(&capture(undelegated(click)), true);

    #[derive(Clone)]
    struct OptionsProbe(Rc<Cell<usize>>);

    impl EventDescriptor for OptionsProbe {
        type EventType = web_sys::Event;
        const BUBBLES: bool = true;
        const CAPTURE: bool = true;

        fn name(&self) -> Cow<'static, str> {
            "options-probe".into()
        }

        fn event_delegation_key(&self) -> Cow<'static, str> {
            "$$$options-probe".into()
        }

        fn options(&self) -> Option<&web_sys::AddEventListenerOptions> {
            self.0.set(self.0.get() + 1);
            None
        }
    }

    let calls = Rc::new(Cell::new(0));
    let probe = undelegated(OptionsProbe(Rc::clone(&calls)));
    assert_direct(&probe, true);
    assert_eq!(probe.name(), "options-probe");
    assert_eq!(probe.event_delegation_key(), "$$$options-probe");
    assert!(probe.options().is_none());
    assert_eq!(calls.get(), 1);
}

#[test]
fn undelegated_event_modifiers_compile_on_elements_and_components() {
    let _view = view! { <button on:click:undelegated=|_: web_sys::MouseEvent| {} /> };
    let _view = view! { <div on:click:target:capture:undelegated=|_| {} /> };
    let _view = view! { <div on:custom-event:undelegated=|_: web_sys::Event| {} /> };

    #[component]
    fn Target() -> impl IntoView {
        view! { <div /> }
    }

    let _view = view! {
        <Target on:click:undelegated=|_: web_sys::MouseEvent| {} />
        <Target on:custom-event:capture:undelegated=|_: web_sys::Event| {} />
    };
}

#[test]
fn sibling_context_preserves_the_svg_namespace() {
    Owner::new().with(|| {
        let single = NodeRef::<web_interface::svg::Title>::new();
        let _view = view! { <svg><title node_ref=single /></svg> };

        let multiple = NodeRef::<web_interface::svg::Title>::new();
        let _view = view! { <svg><title node_ref=multiple /><circle /></svg> };

        let fragment = NodeRef::<web_interface::svg::Title>::new();
        let _view = view! { <svg><><title node_ref=fragment /><circle /></></svg> };
    });
}

#[test]
fn sibling_context_collects_component_slots_in_source_order() {
    use std::sync::{Arc, Mutex};

    #[slot]
    struct ItemSlot {
        value: &'static str,
    }

    #[component]
    fn SlotReceiver(
        item_slot: Vec<ItemSlot>,
        captured: Arc<Mutex<Vec<&'static str>>>,
    ) -> impl IntoView {
        *captured.lock().unwrap() = item_slot.into_iter().map(|slot| slot.value).collect();
    }

    let captured = Arc::new(Mutex::new(Vec::new()));
    Owner::new().with(|| {
        let _view = view! {
            <SlotReceiver captured=captured.clone()>
                <ItemSlot slot value="first" />
                <><ItemSlot slot value="second" /></>
            </SlotReceiver>
        };
    });
    assert_eq!(*captured.lock().unwrap(), ["first", "second"]);
}

#[test]
fn inert_attribute_literals_keep_the_inner_html_consumer_type() {
    fn same_type<T>(_: T, _: T) {}

    same_type(
        view! { <div><div inner_html="<b>content</b>" /></div> },
        view! { <div><div inner_html={"<b>content</b>"} /></div> },
    );
    same_type(
        view! { <svg><g inner_html="<path />" /></svg> },
        view! { <svg><g inner_html={"<path />"} /></svg> },
    );
}

#[test]
fn inert_namespace_ambiguous_svg_children_use_svg_constructors() {
    fn same_type<T>(_: T, _: T) {}

    same_type(
        view! { <svg><title>"label"</title></svg> },
        View::new(
            web_interface::svg::svg().child(web_interface::svg::InertElement::new(
                "<title>label</title>",
            )),
        ),
    );
    same_type(
        view! { <svg><a href="#target">"link"</a></svg> },
        View::new(
            web_interface::svg::svg().child(web_interface::svg::InertElement::new(
                "<a href=\"#target\">link</a>",
            )),
        ),
    );
}

#[test]
fn inert_namespace_ordinary_html_and_svg_keep_their_constructors() {
    fn same_type<T>(_: T, _: T) {}

    same_type(
        view! { <div><a href="#target">"link"</a></div> },
        View::new(web_interface::html::div().child(
            web_interface::view_renderer::html::InertElement::new("<a href=\"#target\">link</a>"),
        )),
    );
    same_type(
        view! { <svg><g id="group"><title>"label"</title></g></svg> },
        View::new(
            web_interface::svg::svg().child(web_interface::svg::InertElement::new(
                "<g id=\"group\"><title>label</title></g>",
            )),
        ),
    );
}
