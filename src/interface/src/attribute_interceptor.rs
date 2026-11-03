use crate::attr::{
    Attribute, NextAttribute,
    any_attribute::{AnyAttribute, IntoAnyAttribute},
};
use web_interface::prelude::*;

/// Function stored to build/rebuild the wrapped children when attributes are added.
type ChildBuilder<T> = dyn Fn(AnyAttribute) -> T + Send + Sync + 'static;

/// Intercepts attributes passed to your component, allowing passing them to any element.
///
/// By default, web-interface passes any attributes passed to your component (e.g. `<MyComponent
/// attr:class="some-class"/>`) to the top-level element in the view returned by your component.
/// [`AttributeInterceptor`] allows you to intercept this behavior and pass it onto any element in
/// your component instead.
///
/// Must be the top level element in your component's view.
///
/// ## Example
///
/// Any attributes passed to MyComponent will be passed to the #inner element.
///
/// ```
/// # use web_interface::prelude::*;
/// use web_interface::attribute_interceptor::AttributeInterceptor;
///
/// #[component]
/// pub fn MyComponent() -> impl IntoView {
///     view! {
///         <AttributeInterceptor let:attrs>
///             <div id="wrapper">
///                 <div id="inner" {..attrs} />
///             </div>
///         </AttributeInterceptor>
///     }
/// }
/// ```
#[component(transparent)]
pub fn AttributeInterceptor<Chil, T>(
    /// The elements that will be rendered, with the attributes this component received as a
    /// parameter.
    children: Chil,
) -> impl IntoView
where
    Chil: Fn(AnyAttribute) -> T + Send + Sync + 'static,
    T: IntoView + 'static,
{
    AttributeInterceptorInner::new(children)
}

/// Wrapper to intercept attributes passed to a component so you can apply them to a different
/// element.
struct AttributeInterceptorInner<T: IntoView, A> {
    children_builder: Box<ChildBuilder<T>>,
    children: T,
    attributes: A,
}

impl<T: IntoView> AttributeInterceptorInner<T, ()> {
    /// Use this as the returned view from your component to collect the attributes that are passed
    /// to your component so you can manually handle them.
    pub fn new<F>(children: F) -> Self
    where
        F: Fn(AnyAttribute) -> T + Send + Sync + 'static,
    {
        let children_builder = Box::new(children);
        let children = children_builder(().into_any_attr());

        Self {
            children_builder,
            children,
            attributes: (),
        }
    }
}

impl<T: IntoView, A: Attribute> Render for AttributeInterceptorInner<T, A> {
    type State = <T as Render>::State;

    fn build(self) -> Self::State {
        self.children.build()
    }

    fn rebuild(self, state: &mut Self::State) {
        self.children.rebuild(state);
    }
}

impl<T: IntoView + 'static, A> AddAnyAttr for AttributeInterceptorInner<T, A>
where
    A: Attribute,
{
    type Output<SomeNewAttr: web_interface::attr::Attribute> = AttributeInterceptorInner<
        T,
        <<A as NextAttribute>::Output<SomeNewAttr> as Attribute>::CloneableOwned,
    >;

    fn add_any_attr<NewAttr: web_interface::attr::Attribute>(
        self,
        attr: NewAttr,
    ) -> Self::Output<NewAttr>
    where
        Self::Output<NewAttr>: RenderHtml,
    {
        let attributes = self.attributes.add_any_attr(attr).into_cloneable_owned();

        let children = (self.children_builder)(attributes.clone().into_any_attr());

        AttributeInterceptorInner {
            children_builder: self.children_builder,
            children,
            attributes,
        }
    }
}

impl<T: IntoView + 'static, A: Attribute> RenderHtml for AttributeInterceptorInner<T, A> {
    type Owned = AttributeInterceptorInner<T, A::CloneableOwned>;

    const EXISTS: bool = T::EXISTS;

    fn hydrate(
        self,
        cursor: &web_interface::view_renderer::hydration::Cursor,
        position: &web_interface::view_renderer::view::PositionState,
    ) -> Self::State {
        self.children.hydrate(cursor, position)
    }

    fn into_owned(self) -> Self::Owned {
        AttributeInterceptorInner {
            children_builder: self.children_builder,
            children: self.children,
            attributes: self.attributes.into_cloneable_owned(),
        }
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/attribute_interceptor/tests.rs"]
mod tests;
