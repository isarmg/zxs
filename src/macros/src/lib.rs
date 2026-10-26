//! Macros for use with the web-interface framework.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use component::DummyModel;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenTree};
use quote::quote;
use rust_syntax::{spanned::Spanned, token::Pub, Visibility};
use std::str::FromStr;

mod view;
use crate::component::unmodified_fn_name_from_fn_name;
mod component;
mod diagnostics;
mod memo;
mod prop_options;
mod slice;
mod slot;
mod view_source;

/// The `view` macro uses RSX (like JSX, but Rust!) It follows most of the
/// same rules as HTML, with the following differences:
///
/// 1. Text content should be provided as a Rust string, i.e., double-quoted:
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// view! { <p>"Here’s some text"</p> }
/// # }
/// ```
///
/// 2. Self-closing tags need an explicit `/` as in XML/XHTML
/// ```rust,compile_fail
/// # use web_interface::prelude::*;
///
/// # fn test() -> impl IntoView {
/// // ❌ not like this
/// view! { <input type="text" name="name"> }
/// # ;
/// # }
/// ```
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// // ✅ add that slash
/// view! { <input type="text" name="name" /> }
/// # }
/// ```
///
/// 3. Components (functions annotated with `#[component]`) can be inserted as PascalCased tags. (Generics
///    on components are specified as `<Component<T>/>`, not the turbofish `<Component::<T>/>`.)
/// ```rust
/// # use web_interface::prelude::*;
///
/// # #[component]
/// # fn Counter(initial_value: i32) -> impl IntoView { view! { <p></p>} }
/// # fn test() -> impl IntoView {
/// view! { <div><Counter initial_value=3 /></div> }
/// # ;
/// # }
/// ```
///
/// 4. Dynamic content can be wrapped in curly braces (`{ }`) to insert text nodes, elements, or set attributes.
///    If you insert a signal here, web-interface will create an effect to update the DOM whenever the value changes.
///    *(“Signal” here means `Fn() -> T` where `T` is the appropriate type for that node: a `String` in case
///    of text nodes, a `bool` for `class:` attributes, etc.)*
///
///    Attributes can take a wide variety of primitive types that can be converted to strings. They can also
///    take an `Option`, in which case `Some` sets the attribute and `None` removes the attribute.
///
///    Note that in some cases, rust-analyzer support may be better if attribute values are surrounded with braces (`{}`).
///    Unlike in JSX, attribute values are not required to be in braces, but braces can be used and may improve this LSP support.
///
/// ```rust
/// # use web_interface::prelude::*;
///
/// # fn test() -> impl IntoView {
/// let (count, set_count) = signal(0);
///
/// view! {
///   // ❌ not like this: `count.get()` returns an `i32`, not a function
///   <p>{count.get()}</p>
///   // ✅ this is good: web-interface sees the function and knows it's a dynamic value
///   <p>{move || count.get()}</p>
/// }
/// # ;
/// # };
/// ```
///
/// 5. Event handlers can be added with `on:` attributes. In most cases, the events are given the correct type
///    based on the event name.
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// view! {
///   <button on:click=|ev| {
///     web_interface::logging::log!("click event: {ev:#?}");
///   }>
///     "Click me"
///   </button>
/// }
/// # }
/// ```
///
/// 6. DOM properties can be set with `prop:` attributes, which take any primitive type or `JsValue` (or a signal
///    that returns a primitive or JsValue). They can also take an `Option`, in which case `Some` sets the property
///    and `None` deletes the property.
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let (name, set_name) = signal("Alice".to_string());
///
/// view! {
///   <input
///     type="text"
///     name="user_name"
///     value={move || name.get()} // this only sets the default value!
///     prop:value={move || name.get()} // here's how you update values. Sorry, I didn’t invent the DOM.
///     on:click=move |ev| set_name.set(event_target_value(&ev)) // `event_target_value` is a useful little web-interface helper
///   />
/// }
/// # }
/// ```
///
/// 7. Classes can be toggled with `class:` attributes, which take a `bool` (or a signal that returns a `bool`).
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let (count, set_count) = signal(2);
/// view! { <div class:hidden-div={move || count.get() < 3}>"Now you see me, now you don’t."</div> }
/// # }
/// ```
///
/// Class names can include dashes, and since v0.5.0 can include a dash-separated segment of only numbers.
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let (count, set_count) = signal(2);
/// view! { <div class:hidden-div-25={move || count.get() < 3}>"Now you see me, now you don’t."</div> }
/// # }
/// ```
///
/// Class names cannot include special symbols.
/// ```rust,compile_fail
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let (count, set_count) = signal(2);
/// // class:hidden-[div]-25 is invalid attribute name
/// view! { <div class:hidden-[div]-25={move || count.get() < 3}>"Now you see me, now you don’t."</div> }
/// # }
/// ```
///
/// However, you can pass arbitrary class names using the syntax `class=("name", value)`.
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let (count, set_count) = signal(2);
/// // this allows you to use CSS frameworks that include complex class names
/// view! {
///   <div
///     class=("is-[this_-_really]-necessary-42", move || count.get() < 3)
///   >
///     "Now you see me, now you don’t."
///   </div>
/// }
/// # }
/// ```
///
/// 8. Individual styles can also be set with `style:` or `style=("property-name", value)` syntax.
/// ```rust
/// # use web_interface::prelude::*;
///
/// # fn test() -> impl IntoView {
/// let (x, set_x) = signal(0);
/// let (y, set_y) = signal(0);
/// view! {
///   <div
///     style="position: absolute"
///     style:left=move || format!("{}px", x.get())
///     style:top=move || format!("{}px", y.get())
///     style=("background-color", move || format!("rgb({}, {}, 100)", x.get(), y.get()))
///   >
///     "Moves when coordinates change"
///   </div>
/// }
/// # }
/// ```
///
/// 9. You can use the `node_ref` or `_ref` attribute to store a reference to its DOM element in a
///    `NodeRef` to use later.
/// ```rust
/// # use web_interface::prelude::*;
///
/// # fn test() -> impl IntoView {
/// use web_interface::html::Input;
///
/// let (value, set_value) = signal(0);
/// let my_input = NodeRef::<Input>::new();
/// view! { <input type="text" node_ref=my_input/> }
/// // `my_input` now contains an `Element` that we can use anywhere
/// # ;
/// # };
/// ```
///
/// 10. You can add the same class to every element in the view by passing in a special
///     `class = {/* ... */},` argument after ``. This is useful for injecting a class
///     provided by a scoped styling library.
/// ```rust
/// # use web_interface::prelude::*;
///
/// # fn test() -> impl IntoView {
/// let class = "mycustomclass";
/// view! { class = class,
///   <div> // will have class="mycustomclass"
///     <p>"Some text"</p> // will also have class "mycustomclass"
///   </div>
/// }
/// # }
/// ```
///
/// 11. You can set any HTML element’s `innerHTML` with the `inner_html` attribute on an
///     element. Be careful: this HTML will not be escaped, so you should ensure that it
///     only contains trusted input.
/// ```rust
/// # use web_interface::prelude::*;
/// # fn test() -> impl IntoView {
/// let html = "<p>This HTML will be injected.</p>";
/// view! {
///   <div inner_html=html/>
/// }
/// # }
/// ```
///
/// Here’s a simple example that shows off several of these features, put together
/// ```rust
/// # use web_interface::prelude::*;
/// pub fn SimpleCounter() -> impl IntoView {
///     // create a reactive signal with the initial value
///     let (value, set_value) = signal(0);
///
///     // create event handlers for our buttons
///     // note that `value` and `set_value` are `Copy`, so it's super easy to move them into closures
///     let clear = move |_ev| set_value.set(0);
///     let decrement = move |_ev| set_value.update(|value| *value -= 1);
///     let increment = move |_ev| set_value.update(|value| *value += 1);
///
///     view! {
///         <div>
///             <button on:click=clear>"Clear"</button>
///             <button on:click=decrement>"-1"</button>
///             <span>"Value: " {move || value.get().to_string()} "!"</span>
///             <button on:click=increment>"+1"</button>
///         </div>
///     }
/// }
/// ```
#[proc_macro]
pub fn view(tokens: TokenStream) -> TokenStream {
    view_macro_output(view_macro_impl(tokens, false))
}

/// The `template` macro behaves like [`view`](view!), except that it wraps the entire tree in a
/// `ViewTemplate`. This optimizes creation speed by rendering
/// most of the view into a `<template>` tag with HTML rendered at compile time, then hydrating it.
/// In exchange, there is a small binary size overhead.
///
/// ```rust
/// # use web_interface::prelude::*;
/// fn Greeting() -> impl IntoView {
///     template! { <p>"Hello from a browser template"</p> }
/// }
/// ```
#[proc_macro]
pub fn template(tokens: TokenStream) -> TokenStream {
    view_macro_output(view_macro_impl(tokens, true))
}

fn view_macro_output(result: rust_syntax::Result<proc_macro2::TokenStream>) -> TokenStream {
    result
        .unwrap_or_else(|error| {
            let errors = error.into_compile_error();
            // A view macro expands in expression position. Keeping combined
            // errors inside one valid block expression lets rustc report each
            // one without parser follow-on errors.
            quote!({ #errors })
        })
        .into()
}

fn macro_output(result: rust_syntax::Result<proc_macro2::TokenStream>) -> TokenStream {
    result
        .unwrap_or_else(rust_syntax::Error::into_compile_error)
        .into()
}

fn view_macro_impl(
    tokens: TokenStream,
    template: bool,
) -> rust_syntax::Result<proc_macro2::TokenStream> {
    let tokens: proc_macro2::TokenStream = tokens.into();
    let mut tokens = tokens.into_iter();

    let first = tokens.next();
    let second = tokens.next();
    let third = tokens.next();
    let fourth = tokens.next();
    let global_class = match (&first, &second) {
        (Some(TokenTree::Ident(first)), Some(TokenTree::Punct(eq)))
            if *first == "class" && eq.as_char() == '=' =>
        {
            match &fourth {
                Some(TokenTree::Punct(comma)) if comma.as_char() == ',' => third.clone(),
                _ => {
                    return Err(rust_syntax::Error::new(
                        second.span(),
                        diagnostics::message_with_help(
                            "To create a scope class with the view! macro you \
                             must put a comma `,` after the value",
                            r#"e.g., view!{ class="my-class", <div>...</div>}"#,
                        ),
                    ));
                }
            }
        }
        _ => None,
    };
    let tokens = if global_class.is_some() {
        tokens.collect::<proc_macro2::TokenStream>()
    } else {
        [first, second, third, fourth]
            .into_iter()
            .flatten()
            .chain(tokens)
            .collect()
    };
    let config = rstml::ParserConfig::default().recover_block(true);
    let parser = rstml::Parser::new(config);
    let (mut nodes, errors) = parser.parse_recoverable(tokens).split_vec();
    let errors = errors.into_iter().map(|e| e.emit_as_expr_tokens());
    let nodes_output = view::render_view(
        &mut nodes,
        global_class.as_ref(),
        normalized_call_site(proc_macro::Span::call_site()),
        template,
    )?;

    // The allow lint needs to be put here instead of at the expansion of
    // view::attribute_value(). Adding this next to the expanded expression
    // seems to break rust-analyzer, but it works when the allow is put here.
    let output = quote! {
        {
            #[allow(unused_braces)]
            {
                #(#errors;)*
                #nodes_output
            }
        }
    };

    Ok(if template {
        quote! {
            ::web_interface::prelude::ViewTemplate::new(#output)
        }
    } else {
        output
    })
}

fn normalized_call_site(site: proc_macro::Span) -> Option<String> {
    if cfg!(debug_assertions) {
        Some(view_source::span_to_stable_id(
            site.file(),
            site.start().line(),
        ))
    } else {
        _ = site;
        None
    }
}

/// This behaves like the [`view`](view!) macro, but loads the view from an external file instead of
/// parsing it inline.
///
/// This is designed to allow editing views in a separate file, if this improves a user's workflow.
///
/// The file is loaded and parsed during proc-macro execution, and its path is resolved relative to
/// the consumer crate root rather than relative to the file from which it is called.
/// Changes to the view file are tracked by the compiler just like `include_str!` inputs.
#[proc_macro]
pub fn include_view(tokens: TokenStream) -> TokenStream {
    view_macro_output(include_view_impl(tokens))
}

fn include_view_impl(tokens: TokenStream) -> rust_syntax::Result<proc_macro2::TokenStream> {
    let file_name = rust_syntax::parse::<rust_syntax::LitStr>(tokens).map_err(|mut error| {
        error.combine(rust_syntax::Error::new(
            Span::call_site(),
            "the only supported argument is a string literal",
        ));
        error
    })?;
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        rust_syntax::Error::new(
            file_name.span(),
            "include_view! requires CARGO_MANIFEST_DIR",
        )
    })?;
    let path = std::path::PathBuf::from(manifest_dir).join(file_name.value());
    let path = path.to_str().ok_or_else(|| {
        rust_syntax::Error::new(file_name.span(), "the view file path must be valid UTF-8")
    })?;
    let file = std::fs::read_to_string(path).map_err(|error| {
        rust_syntax::Error::new(
            file_name.span(),
            format!("could not read view file: {error}"),
        )
    })?;
    let tokens = proc_macro2::TokenStream::from_str(&file)
        .map_err(|e| rust_syntax::Error::new(Span::call_site(), e))?;
    let view = view_macro_impl(tokens.into(), false)?;
    Ok(quote! {{
        const _: &str = include_str!(#path);
        #view
    }})
}

/// Annotates a function so that it can be used with your template as a web-interface `<Component/>`.
///
/// The `#[component]` macro allows you to annotate plain Rust functions as components
/// and use them within your web-interface [view](crate::view!) as if they were custom HTML elements. The
/// component function takes any number of other arguments. When you use the component somewhere else,
/// the names of its arguments are the names of the properties you use in the [view](crate::view!) macro.
///
/// Every component function should have the return type `-> impl IntoView`.
///
/// You can add Rust doc comments to component function arguments and the macro will use them to
/// generate documentation for the component.
///
/// Here’s how you would define and use a simple web-interface component which can accept custom properties for a name and age:
///
/// ```rust
/// # use web_interface::prelude::*;
///
/// #[component]
/// fn HelloComponent(
///     /// The user's name.
///     name: String,
///     /// The user's age.
///     age: u8,
/// ) -> impl IntoView {
///     view! {
///       <p>"Your name is " {name} " and you are " {age} " years old."</p>
///     }
/// }
///
/// #[component]
/// fn App() -> impl IntoView {
///     view! {
///       <main>
///         <HelloComponent name="Greg".to_string() age=32/>
///       </main>
///     }
/// }
/// ```
///
/// Here are some important details about how web-interface components work within the framework:
///
/// * **The component function only runs once.** Your component function is not a “render” function
///   that re-runs whenever changes happen in the state. It’s a “setup” function that runs once to
///   create the user interface, and sets up a reactive system to update it. This means it’s okay
///   to do relatively expensive work within the component function, as it will only happen once,
///   not on every state change.
///
/// * Component names are usually in `PascalCase`. If you use a `snake_case` name, then the generated
///   component's name will still be in `PascalCase`. This is how the framework recognizes that
///   a particular tag is a component, not an HTML element.
///
/// ```
/// # use web_interface::prelude::*;
/// // PascalCase: Generated component will be called MyComponent
/// #[component]
/// fn MyComponent() -> impl IntoView {}
///
/// // snake_case: Generated component will be called MySnakeCaseComponent
/// #[component]
/// fn my_snake_case_component() -> impl IntoView {}
/// ```
///
/// 5. You can access the children passed into the component with the `children` property, which takes
///    an argument of the type `Children`. This is an alias for `Box<dyn FnOnce() -> AnyView + Send>`.
///    If you need `children` to be a `Fn` or `FnMut`, you can use the `ChildrenFn` or `ChildrenFnMut`
///    type aliases. If you want to iterate over the children, you can take `ChildrenFragment`.
///
/// ```
/// # use web_interface::prelude::*;
/// #[component]
/// fn ComponentWithChildren(children: ChildrenFragment) -> impl IntoView {
///     view! {
///       <ul>
///         {children()
///           .nodes
///           .into_iter()
///           .map(|child| view! { <li>{child}</li> })
///           .collect::<Vec<_>>()}
///       </ul>
///     }
/// }
///
/// #[component]
/// fn WrapSomeChildren() -> impl IntoView {
///     view! {
///       <ComponentWithChildren>
///         "Ooh, look at us!"
///         <span>"We're being projected!"</span>
///       </ComponentWithChildren>
///     }
/// }
/// ```
///
/// ## Customizing Properties
///
/// You can use the `#[prop]` attribute on individual component properties (function arguments) to
/// customize the types that component property can receive. You can use the following attributes:
///
/// * `#[prop(into)]`: This will call `.into()` on any value passed into the component prop. (For example,
///   you could apply `#[prop(into)]` to a prop that takes
///   `Signal`, which would
///   allow users to pass a `ReadSignal` or
///   `RwSignal`
///   and automatically convert it.)
/// * `#[prop(optional)]`: If the user does not specify this property when they use the component,
///   it will be set to its default value. If the property type is `Option<T>`, values should be passed
///   as `name=T` and will be received as `Some(T)`.
/// * `#[prop(optional_no_strip)]`: The same as `optional`, but requires values to be passed as `None` or
///   `Some(T)` explicitly. This means that the optional property can be omitted (and be `None`), or explicitly
///   specified as either `None` or `Some(T)`.
/// * `#[prop(default = <expr>)]`: Optional property that specifies a default value, which is used when the
///   property is not specified.
/// * `#[prop(name = "new_name")]`: Specify a different name for the property. Can be used to destructure
///   fields in component function parameters (see example below).
/// * `#[prop(marker)]`: Specify that this prop is a default-only marker, which should be omitted from the docs
///   and the builder. It is useful when you want a generic component where no props actually use that generic;
///   you can then use `#[prop(marker)] _marker: PhantomData<T>`.
///
/// ```rust
/// # use web_interface::prelude::*;
///
/// #[component]
/// pub fn MyComponent(
///     #[prop(into)] name: String,
///     #[prop(optional)] optional_value: Option<i32>,
///     #[prop(optional_no_strip)] optional_no_strip: Option<i32>,
///     #[prop(default = 7)] optional_default: i32,
///     #[prop(name = "data")] UserInfo { email, user_id }: UserInfo,
/// ) -> impl IntoView {
///     // whatever UI you need
/// }
///
/// #[component]
/// pub fn App() -> impl IntoView {
///     view! {
///       <MyComponent
///         name="Greg"  // automatically converted to String with `.into()`
///         optional_value=42  // received as `Some(42)`
///         optional_no_strip=Some(42)  // received as `Some(42)`
///         optional_default=42  // received as `42`
///         data=UserInfo {email: "foo", user_id: "bar" }
///       />
///       <MyComponent
///         name="Bob" // automatically converted to String with `.into()`
///         data=UserInfo {email: "foo", user_id: "bar" }
///         // optional values can be omitted
///       />
///     }
/// }
///
/// pub struct UserInfo {
///     pub email: &'static str,
///     pub user_id: &'static str,
/// }
/// ```
#[proc_macro_attribute]
pub fn component(args: proc_macro::TokenStream, s: TokenStream) -> TokenStream {
    macro_output(component_impl(args, s))
}

fn component_impl(
    args: proc_macro::TokenStream,
    s: TokenStream,
) -> rust_syntax::Result<proc_macro2::TokenStream> {
    let is_transparent = if !args.is_empty() {
        let transparent = rust_syntax::parse::<rust_syntax::Ident>(args)?;

        if transparent != "transparent" {
            return Err(rust_syntax::Error::new_spanned(
                transparent,
                diagnostics::message_with_help(
                    "only `transparent` is supported",
                    "try `#[component(transparent)]` or `#[component]`",
                ),
            ));
        }

        true
    } else {
        false
    };

    component_macro(s, is_transparent)
}

fn component_macro(
    s: TokenStream,
    is_transparent: bool,
) -> rust_syntax::Result<proc_macro2::TokenStream> {
    let mut dummy = rust_syntax::parse::<DummyModel>(s.clone());
    let parse_result = rust_syntax::parse::<component::Model>(s);

    if let (Ok(ref mut unexpanded), Ok(model)) = (&mut dummy, parse_result) {
        let mut errors = diagnostics::Errors::default();
        let expanded = model.is_transparent(is_transparent).expand(&mut errors);
        if !matches!(unexpanded.vis, Visibility::Public(_)) {
            unexpanded.vis = Visibility::Public(Pub {
                span: unexpanded.vis.span(),
            })
        }
        unexpanded.sig.ident = unmodified_fn_name_from_fn_name(&unexpanded.sig.ident);

        let output = expanded.map(|expanded| {
            quote! {
                #expanded

                #[doc(hidden)]
                #[allow(clippy::too_many_arguments, clippy::needless_lifetimes)]
                #unexpanded
            }
        });
        errors.finish(output)
    } else {
        dummy.map(|mut dummy| {
            dummy.sig.ident = unmodified_fn_name_from_fn_name(&dummy.sig.ident);
            quote! {
                #[doc(hidden)]
                #[allow(clippy::too_many_arguments, clippy::needless_lifetimes)]
                #dummy
            }
        })
    }
}

/// Annotates a struct so that it can be used with your Component as a `slot`.
///
/// The `#[slot]` macro allows you to annotate plain Rust struct as component slots and use them
/// within your web-interface [`component`](macro@crate::component) properties. The struct can contain any number
/// of fields. When you use the component somewhere else, the names of the slot fields are the
/// names of the properties you use in the [view](crate::view!) macro.
///
/// Here’s how you would define and use a simple web-interface component which can accept a custom slot:
/// ```rust
/// # use web_interface::prelude::*;
/// #[slot]
/// struct HelloSlot {
///     // Same prop syntax as components.
///     #[prop(optional)]
///     children: Option<Children>,
/// }
///
/// #[component]
/// fn HelloComponent(
///     /// Component slot, should be passed through the <HelloSlot slot> syntax.
///     hello_slot: HelloSlot,
/// ) -> impl IntoView {
///     hello_slot.children.map(|children| children())
/// }
///
/// #[component]
/// fn App() -> impl IntoView {
///     view! {
///         <HelloComponent>
///             <HelloSlot slot>
///                 "Hello, World!"
///             </HelloSlot>
///         </HelloComponent>
///     }
/// }
/// ```
///
/// Here are some important details about how slots work within the framework:
/// 1. Most of the same rules from [`macro@component`] macro should also be followed on slots.
///
/// 2. Specifying only `slot` without a name (such as in `<HelloSlot slot>`) will default the chosen slot to
///    the a snake case version of the slot struct name (`hello_slot` for `<HelloSlot>`).
///
/// 3. Event handlers cannot be specified directly on the slot.
///
/// ```rust,compile_fail
/// // ❌ This won't work
/// # use web_interface::prelude::*;
///
/// #[slot]
/// struct SlotWithChildren {
///     children: Children,
/// }
///
/// #[component]
/// fn ComponentWithSlot(slot: SlotWithChildren) -> impl IntoView {
///     (slot.children)()
/// }
///
/// #[component]
/// fn App() -> impl IntoView {
///     view! {
///         <ComponentWithSlot>
///           <SlotWithChildren slot:slot on:click=move |_| {}>
///             <h1>"Hello, World!"</h1>
///           </SlotWithChildren>
///         </ComponentWithSlot>
///     }
/// }
/// ```
///
/// ```
/// // ✅ Do this instead
/// # use web_interface::prelude::*;
///
/// #[slot]
/// struct SlotWithChildren {
///     children: Children,
/// }
///
/// #[component]
/// fn ComponentWithSlot(slot: SlotWithChildren) -> impl IntoView {
///     (slot.children)()
/// }
///
/// #[component]
/// fn App() -> impl IntoView {
///     view! {
///         <ComponentWithSlot>
///           <SlotWithChildren slot:slot>
///             <div on:click=move |_| {}>
///               <h1>"Hello, World!"</h1>
///             </div>
///           </SlotWithChildren>
///         </ComponentWithSlot>
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn slot(args: proc_macro::TokenStream, s: TokenStream) -> TokenStream {
    macro_output(slot_impl(args, s))
}

fn slot_impl(
    args: proc_macro::TokenStream,
    s: TokenStream,
) -> rust_syntax::Result<proc_macro2::TokenStream> {
    if !args.is_empty() {
        return Err(rust_syntax::Error::new(
            Span::call_site(),
            diagnostics::message_with_help("no arguments are supported", "try just `#[slot]`"),
        ));
    }

    rust_syntax::parse::<slot::Model>(s)?.expand()
}

/// Generates a `slice` into a struct with a default getter and setter.
///
/// Can be used to access deeply nested fields within a global state object.
///
/// ```rust
/// # use web_interface::prelude::*;
/// # use view_macros::slice;
///
/// #[derive(Default)]
/// pub struct Outer {
///     count: i32,
///     inner: Inner,
/// }
///
/// #[derive(Default)]
/// pub struct Inner {
///     inner_count: i32,
///     inner_name: String,
/// }
///
/// # let owner = Owner::new();
/// # owner.with(|| {
/// let outer_signal = RwSignal::new(Outer::default());
///
/// let (count, set_count) = slice!(outer_signal.count);
///
/// let (inner_count, set_inner_count) = slice!(outer_signal.inner.inner_count);
/// let (inner_name, set_inner_name) = slice!(outer_signal.inner.inner_name);
/// set_count.set(3);
/// set_inner_count.set(7);
/// set_inner_name.set("Ada".to_string());
/// assert_eq!(count.get(), 3);
/// assert_eq!(inner_count.get(), 7);
/// assert_eq!(inner_name.get(), "Ada");
/// assert_eq!(outer_signal.with(|outer| outer.inner.inner_count), 7);
/// # });
/// ```
#[proc_macro]
pub fn slice(input: TokenStream) -> TokenStream {
    slice::slice_impl(input)
}

/// Generates a `memo` into a struct with a default getter.
///
/// Can be used to access deeply nested fields within a global state object.
///
/// ```rust
/// # use web_interface::prelude::*;
/// # use view_macros::memo;
///
/// #[derive(Default)]
/// pub struct Outer {
///     count: i32,
///     inner: Inner,
/// }
///
/// #[derive(Default)]
/// pub struct Inner {
///     inner_count: i32,
///     inner_name: String,
/// }
///
/// # let owner = Owner::new();
/// # owner.with(|| {
/// let outer_signal = RwSignal::new(Outer::default());
///
/// let count = memo!(outer_signal.count);
///
/// let inner_count = memo!(outer_signal.inner.inner_count);
/// let inner_name = memo!(outer_signal.inner.inner_name);
/// assert_eq!(count.get(), 0);
/// assert_eq!(inner_count.get(), 0);
/// assert_eq!(inner_name.get(), "");
/// outer_signal.update(|outer| {
///     outer.count = 3;
///     outer.inner.inner_count = 7;
///     outer.inner.inner_name = "Ada".to_string();
/// });
/// assert_eq!(count.get(), 3);
/// assert_eq!(inner_count.get(), 7);
/// assert_eq!(inner_name.get(), "Ada");
/// # });
/// ```
#[proc_macro]
pub fn memo(input: TokenStream) -> TokenStream {
    memo::memo_impl(input)
}

#[cfg(test)]
#[path = "../../../test/macros/unit/lib/documentation_diagnostics.rs"]
mod documentation_diagnostics;
