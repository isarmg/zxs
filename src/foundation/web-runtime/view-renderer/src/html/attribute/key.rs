use super::{Attr, AttributeValue};
use std::fmt::Debug;

/// An HTML attribute key.
pub trait AttributeKey: Clone + Send + 'static {
    /// The name of the attribute.
    const KEY: &'static str;
}

macro_rules! attributes {
	($(#[$meta:meta] $key:ident $html:literal),* $(,)?) => {
        paste::paste! {
            $(
                #[$meta]
                #[track_caller]
                pub fn $key<V>(value: V) -> Attr<[<$key:camel>], V>
				where V: AttributeValue,

                {
                    Attr([<$key:camel>], value)
                }

                #[$meta]
				#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
				pub struct [<$key:camel>];

				impl AttributeKey for [<$key:camel>] {
					const KEY: &'static str = $html;
				}
            )*
		}
    }
}

attributes! {
    // HTML

    // ARIA
    /// The `aria-activedescendant` attribute identifies the currently active element when DOM focus is on a composite widget, textbox, group, or application.
    aria_activedescendant "aria-activedescendant",
    /// The `aria-atomic` attribute indicates whether assistive technologies will present all, or only parts of, the changed region based on the change notifications defined by the aria-relevant attribute.
    aria_atomic "aria-atomic",

    /// The `aria-busy` attribute indicates whether an element, and its subtree, are currently being updated.
    aria_busy "aria-busy",

    /// The `aria-controls` attribute identifies the element (or elements) whose contents or presence are controlled by the current element.
    aria_controls "aria-controls",
    /// The `aria-current` attribute indicates the element representing the current item within a container or set of related elements.
    aria_current "aria-current",

    /// The `aria-disabled` attribute indicates that the element is perceivable but disabled, so it is not editable or otherwise operable.
    aria_disabled "aria-disabled",

    /// The `aria-expanded` attribute indicates whether an element, or another grouping element it controls, is currently expanded or collapsed.
    aria_expanded "aria-expanded",

    /// The `aria-haspopup` attribute indicates the availability and type of interactive popup element, such as menu or dialog, that can be triggered by an element.
    aria_haspopup "aria-haspopup",
    /// The `aria-hidden` attribute indicates whether the element is exposed to an accessibility API.
    aria_hidden "aria-hidden",
    /// The `aria-invalid` attribute indicates the entered value does not conform to the format expected by the application.
    aria_invalid "aria-invalid",

    /// The `aria-label` attribute defines a string value that labels the current element.
    aria_label "aria-label",
    /// The `aria-labelledby` attribute identifies the element (or elements) that labels the current element.
    aria_labelledby "aria-labelledby",
    /// The `aria-live` attribute indicates that an element will be updated, and describes the types of updates the user agents, assistive technologies, and user can expect from the live region.
    aria_live "aria-live",

    /// The `aria-pressed` attribute indicates the current "pressed" state of toggle buttons.
    aria_pressed "aria-pressed",

    /// The `aria-selected` attribute indicates the current "selected" state of various widgets.
    aria_selected "aria-selected",

    /// The `autocapitalize` attribute controls whether and how text input is automatically capitalized as it is entered/edited by the user.
    autocapitalize "autocapitalize",
    /// The `autocomplete` attribute indicates whether an input field can have its value automatically completed by the browser.
    autocomplete "autocomplete",

    /// The `checked` attribute indicates whether an input element is checked or not.
    checked "checked",

    // class is handled in ../class.rs instead
    //class "class",

    /// The `data` attribute specifies the URL of the resource that is being embedded.
    data "data",

    /// The `dir` attribute specifies the text direction for the content in an element.
    dir "dir",

    /// The `disabled` attribute indicates whether the element is disabled.
    disabled "disabled",

    /// The `for` attribute specifies which form element a label is bound to.
    r#for "for",

    /// The `headers` attribute specifies the headers associated with the element.
    headers "headers",
    /// The `height` attribute specifies the height of an element.
    height "height",
    /// The `hidden` attribute indicates that the element is not yet, or is no longer, relevant.
    hidden "hidden",

    /// The `href` attribute specifies the URL of a linked resource.
    href "href",

    /// The `icon` attribute specifies the URL of an image to be used as a graphical icon for the element.
    icon "icon",
    /// The `id` attribute specifies a unique id for an element.
    id "id",

    /// The `label` attribute provides a user-readable title for an element.
    label "label",
    /// The `lang` attribute specifies the language of the element's content.
    lang "lang",

    /// The `max` attribute specifies the maximum value for an input element.
    max "max",
    /// The `maxlength` attribute specifies the maximum number of characters that an input element can accept.
    maxlength "maxlength",

    /// The `min` attribute specifies the minimum value for an input element.
    min "min",
    /// The `minlength` attribute specifies the minimum number of characters that an input element can accept.
    minlength "minlength",
    /// The `multiple` attribute indicates whether the user can enter more than one value.
    multiple "multiple",

    /// The `name` attribute specifies the name of the element.
    name "name",

    /// The `novalidate` attribute indicates that the form should not be validated when submitted.
    novalidate "novalidate",

    /// The `placeholder` attribute provides a short hint that describes the expected value of the input element.
    placeholder "placeholder",

    /// The `popover` attribute indicates that an element is a popover and specifies the event that causes the popover to be shown.
    popover "popover",

    /// The `readonly` attribute indicates that the user cannot modify the value of the input element.
    readonly "readonly",

    /// The `required` attribute indicates that the user must fill in the input element before submitting the form.
    required "required",

    /// The `role` attribute defines the role of an element in the context of a web application.
    role "role",

    /// The `rows` attribute specifies the visible number of lines in a text area.
    rows "rows",

    /// The `scope` attribute specifies whether a header cell is a header for a column, row, or group of columns or rows.
    scope "scope",

    /// The `size` attribute specifies the width of the input element.
    size "size",

    /// The `span` attribute defines the number of columns in a `<colgroup>` or the number of rows in a `<rowgroup>`.
    span "span",
    /// The `spellcheck` attribute indicates whether spell checking is allowed for the element.
    spellcheck "spellcheck",

    /// The `start` attribute specifies the start value of the list.
    start "start",

    // style is handled in ../style.rs instead
    // style "style",

    /// The `tabindex` attribute specifies the tab order of an element.
    tabindex "tabindex",
    /// The `target` attribute specifies where to open the linked document.
    target "target",
    /// The `title` attribute provides additional information about an element.
    title "title",

    /// The `type` attribute specifies the type of the element.
    r#type "type",

    /// The `value` attribute specifies the value of the element.
    value "value",

    /// The `width` attribute specifies the width of an element.
    width "width",
    /// The `wrap` attribute specifies how the text in a text area is to be wrapped when submitted in a form.
    wrap "wrap",
    // Event Handler Attributes

    // MathML attributes

    /// The `display` attribute specifies the display style of the element.
    display "display",

}
