use crate::{
    html::{
        attribute::{Attr, Attribute, AttributeValue, NextAttribute},
        element::{ElementType, ElementWithChildren, HtmlElement},
    },
    view::Render,
};
use std::fmt::Debug;

macro_rules! html_element_inner {
    (
        #[$meta:meta]
        $tag:ident
        $struct_name:ident
        $ty:ident
        [$($attr:ty),*]
        $escape:literal
    ) => {
        paste::paste! {
            #[$meta]
            #[track_caller]
            pub fn $tag() -> HtmlElement<$struct_name, (), ()>
            where

            {
                HtmlElement {
                    #[cfg(any(debug_assertions, interface_debuginfo))]
                    defined_at: std::panic::Location::caller(),
                    tag: $struct_name,
                    attributes: (),
                    children: (),
                }
            }

            #[$meta]
            #[derive(Debug, Copy, Clone, PartialEq, Eq)]
            pub struct $struct_name;

            // Typed attribute methods
            impl<At, Ch> HtmlElement<$struct_name, At, Ch>
            where
                At: Attribute,
                Ch: Render,

            {
                $(
                    #[doc = concat!("The [`", stringify!($attr), "`](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/", stringify!($tag), "#", stringify!($attr) ,") attribute on `<", stringify!($tag), ">`.")]
                    pub fn $attr<V>(self, value: V) -> HtmlElement <
                        $struct_name,
                        <At as NextAttribute>::Output<Attr<$crate::html::attribute::[<$attr:camel>], V>>,
                        Ch
                    >
                    where
                        V: AttributeValue,
                        At: NextAttribute,
                        <At as NextAttribute>::Output<Attr<$crate::html::attribute::[<$attr:camel>], V>>: Attribute,
                    {
                        let HtmlElement {
                            #[cfg(any(debug_assertions, interface_debuginfo))]
                            defined_at,
                            tag,
                            children,
                            attributes
                        } = self;
                        HtmlElement {
                            #[cfg(any(debug_assertions, interface_debuginfo))]
                            defined_at,
                            tag,
                            children,
                            attributes: attributes.add_any_attr($crate::html::attribute::$attr(value)),
                        }
                    }
                )*
            }

            impl ElementType for $struct_name {
                type Output = web_sys::$ty;

                const TAG: &'static str = stringify!($tag);
                const SELF_CLOSING: bool = false;
                const ESCAPE_CHILDREN: bool = $escape;
                const NAMESPACE: Option<&'static str> = None;

                #[inline(always)]
                fn tag(&self) -> &str {
                    Self::TAG
                }
            }

            impl ElementWithChildren for $struct_name {}
        }
    };
}

macro_rules! html_elements {
	($(
        #[$meta:meta]
        $tag:ident
        $ty:ident
        [$($attr:ty),*]
        $escape:literal
      ),*
      $(,)?
    ) => {
        paste::paste! {
            $(html_element_inner! {
                #[$meta]
                $tag
                [<$tag:camel>]
                $ty
                [$($attr),*]
                $escape
            })*
        }
    }
}

macro_rules! html_self_closing_elements {
	($(
        #[$meta:meta]
        $tag:ident $ty:ident [$($attr:ty),*] $escape:literal
      ),*
      $(,)?
    ) => {
        paste::paste! {
            $(
                #[$meta]
                #[track_caller]
                pub fn $tag() -> HtmlElement<[<$tag:camel>], (), ()>
                where

                {
                    HtmlElement {
                        #[cfg(any(debug_assertions, interface_debuginfo))]
                        defined_at: std::panic::Location::caller(),
                        attributes: (),
                        children: (),
                        tag: [<$tag:camel>],
                    }
                }

                #[$meta]
                #[derive(Debug, Copy, Clone, PartialEq, Eq)]
                pub struct [<$tag:camel>];

                // Typed attribute methods
                impl<At> HtmlElement<[<$tag:camel>], At, ()>
                where
                    At: Attribute,
                {
                    $(
                        #[doc = concat!("The [`", stringify!($attr), "`](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/", stringify!($tag), "#", stringify!($attr) ,") attribute on `<", stringify!($tag), ">`.")]
                        pub fn $attr<V>(self, value: V) -> HtmlElement<
                            [<$tag:camel>],
                            <At as NextAttribute>::Output<Attr<$crate::html::attribute::[<$attr:camel>], V>>,
                            (),
                        >
                        where
                            V: AttributeValue,
                            At: NextAttribute,
                            <At as NextAttribute>::Output<Attr<$crate::html::attribute::[<$attr:camel>], V>>: Attribute,
                        {
                            let HtmlElement {
                                 #[cfg(any(debug_assertions, interface_debuginfo))]
                                 defined_at,
                                tag,
                                children,
                                attributes,
                            } = self;
                            HtmlElement {
                                #[cfg(any(debug_assertions, interface_debuginfo))]
                                defined_at,
                                tag,
                                children,
                                attributes: attributes.add_any_attr($crate::html::attribute::$attr(value)),
                            }
                        }
                    )*
                }

                impl ElementType for [<$tag:camel>] {
                    type Output = web_sys::$ty;

                    const TAG: &'static str = stringify!($tag);
                    const SELF_CLOSING: bool = true;
                    const ESCAPE_CHILDREN: bool = $escape;
                    const NAMESPACE: Option<&'static str> = None;

                    #[inline(always)]
                    fn tag(&self) -> &str {
                        Self::TAG
                    }
                }
            )*
		}
    }
}

html_self_closing_elements! {

    /// The `<img>` HTML element embeds an image into the document.
    img HtmlImageElement [height, width] true,
    /// The `<input>` HTML element is used to create interactive controls for web-based forms in order to accept data from the user; a wide variety of types of input data and control widgets are available, depending on the device and user agent. The `<input>` element is one of the most powerful and complex in all of HTML due to the sheer number of combinations of input types and attributes.
    input HtmlInputElement [autocomplete, checked, disabled, height, max, maxlength, min, minlength, multiple, name, placeholder, readonly, required, size, r#type, value, width] true,

}

html_elements! {
    /// The `<a>` HTML element (or anchor element), with its href attribute, creates a hyperlink to web pages, files, email addresses, locations in the same page, or anything else a URL can address.
    a HtmlAnchorElement [href, target, r#type] true,

    /// The `<article>` HTML element represents a self-contained composition in a document, page, application, or site, which is intended to be independently distributable or reusable (e.g., in syndication). Examples include: a forum post, a magazine or newspaper article, or a blog entry, a product card, a user-submitted comment, an interactive widget or gadget, or any other independent item of content.
    article HtmlElement [] true,
    /// The `<aside>` HTML element represents a portion of a document whose content is only indirectly related to the document's main content. Asides are frequently presented as sidebars or call-out boxes.
    aside HtmlElement [] true,

    /// The `<b>` HTML element is used to draw the reader's attention to the element's contents, which are not otherwise granted special importance. This was formerly known as the Boldface element, and most browsers still draw the text in boldface. However, you should not use `<b>` for styling text; instead, you should use the CSS font-weight property to create boldface text, or the strong element to indicate that text is of special importance.
    b HtmlElement [] true,

    /// The `<button>` HTML element represents a clickable button, used to submit forms or anywhere in a document for accessible, standard button functionality.
    button HtmlButtonElement [disabled, name, r#type, value] true,

    /// The `<code>` HTML element displays its contents styled in a fashion intended to indicate that the text is a short fragment of computer code. By default, the content text is displayed using the user agent default monospace font.
    code HtmlElement [] true,

    /// The `<dd>` HTML element provides the description, definition, or value for the preceding term (dt) in a description list (dl).
    dd HtmlElement [] true,

    /// The `<details>` HTML element creates a disclosure widget in which information is visible only when the widget is toggled into an "open" state. A summary or label must be provided using the summary element.
    details HtmlDetailsElement [name] true,

    /// The `<dialog>` HTML element represents a dialog box or other interactive component, such as a dismissible alert, inspector, or subwindow.
    dialog HtmlDialogElement [] true,
    /// The `<div>` HTML element is the generic container for flow content. It has no effect on the content or layout until styled in some way using CSS (e.g. styling is directly applied to it, or some kind of layout model like Flexbox is applied to its parent element).
    div HtmlDivElement [] true,
    /// The `<dl>` HTML element represents a description list. The element encloses a list of groups of terms (specified using the dt element) and descriptions (provided by dd elements). Common uses for this element are to implement a glossary or to display metadata (a list of key-value pairs).
    dl HtmlDListElement [] true,
    /// The `<dt>` HTML element specifies a term in a description or definition list, and as such must be used inside a dl element. It is usually followed by a dd element; however, multiple `<dt>` elements in a row indicate several terms that are all defined by the immediate next dd element.
    dt HtmlElement [] true,

    /// The `<fieldset>` HTML element is used to group several controls as well as labels (label) within a web form.
    fieldset HtmlFieldSetElement [disabled, name] true,

    /// The `<form>` HTML element represents a document section containing interactive controls for submitting information.
    form HtmlFormElement [autocomplete, name, novalidate, target] true,
    /// The `<h1>` to `<h6>` HTML elements represent six levels of section headings. `<h1>` is the highest section level and `<h6>` is the lowest.
    h1 HtmlHeadingElement [] true,
    /// The `<h1>` to `<h6>` HTML elements represent six levels of section headings. `<h1>` is the highest section level and `<h6>` is the lowest.
    h2 HtmlHeadingElement [] true,
    /// The `<h1>` to `<h6>` HTML elements represent six levels of section headings. `<h1>` is the highest section level and `<h6>` is the lowest.
    h3 HtmlHeadingElement [] true,

    /// The `<header>` HTML element represents introductory content, typically a group of introductory or navigational aids. It may contain some heading elements but also a logo, a search form, an author name, and other elements.
    header HtmlElement [] true,

    /// The `<html>` HTML element represents the root (top-level element) of an HTML document, so it is also referred to as the root element. All other elements must be descendants of this element.
    html HtmlHtmlElement [] true,

    /// The `<label>` HTML element represents a caption for an item in a user interface.
    label HtmlLabelElement [r#for] true,
    /// The `<legend>` HTML element represents a caption for the content of its parent fieldset.
    legend HtmlLegendElement [] true,
    /// The `<li>` HTML element is used to represent an item in a list. It must be contained in a parent element: an ordered list (ol), an unordered list (ul), or a menu (menu). In menus and unordered lists, list items are usually displayed using bullet points. In ordered lists, they are usually displayed with an ascending counter on the left, such as a number or letter.
    li HtmlLiElement [value] true,
    /// The `<main>` HTML element represents the dominant content of the body of a document. The main content area consists of content that is directly related to or expands upon the central topic of a document, or the central functionality of an application.
    main HtmlElement [] true,

    /// The `<nav>` HTML element represents a section of a page whose purpose is to provide navigation links, either within the current document or to other documents. Common examples of navigation sections are menus, tables of contents, and indexes.
    nav HtmlElement [] true,

    /// The `<p>` HTML element represents a paragraph. Paragraphs are usually represented in visual media as blocks of text separated from adjacent blocks by blank lines and/or first-line indentation, but HTML paragraphs can be any structural grouping of related content, such as images or form fields.
    p HtmlParagraphElement [] true,

    /// The `<pre>` HTML element represents preformatted text which is to be presented exactly as written in the HTML file. The text is typically rendered using a non-proportional, or "monospaced, font. Whitespace inside this element is displayed as written.
    pre HtmlPreElement [] true,

    /// The `<section>` HTML element represents a generic standalone section of a document, which doesn't have a more specific semantic element to represent it. Sections should always have a heading, with very few exceptions.
    section HtmlElement [] true,

    /// The `<span>` HTML element is a generic inline container for phrasing content, which does not inherently represent anything. It can be used to group elements for styling purposes (using the class or id attributes), or because they share attribute values, such as lang. It should be used only when no other semantic element is appropriate. `<span>` is very much like a div element, but div is a block-level element whereas a `<span>` is an inline element.
    span HtmlSpanElement [] true,
    /// The `<strong>` HTML element indicates that its contents have strong importance, seriousness, or urgency. Browsers typically render the contents in bold type.
    strong HtmlElement [] true,

    /// The `<summary>` HTML element specifies a summary, caption, or legend for a details element's disclosure box. Clicking the `<summary>` element toggles the state of the parent `<details>` element open and closed.
    summary HtmlElement [] true,

    /// The `<table>` HTML element represents tabular data — that is, information presented in a two-dimensional table comprised of rows and columns of cells containing data.
    table HtmlTableElement [] true,
    /// The `<tbody>` HTML element encapsulates a set of table rows (tr elements), indicating that they comprise the body of the table (table).
    tbody HtmlTableSectionElement [] true,
    /// The `<td>` HTML element defines a cell of a table that contains data. It participates in the table model.
    td HtmlTableCellElement [headers] true,
    /// The `<template>` HTML element is a mechanism for holding HTML that is not to be rendered immediately when a page is loaded but may be instantiated subsequently during runtime using JavaScript.
    template HtmlTemplateElement [] true,

    /// The `<textarea>` HTML element is a multiline plain-text editing control.
    textarea HtmlTextAreaElement [autocomplete, maxlength, readonly, rows] true,

    /// The `<th>` HTML element defines a cell as header of a group of table cells. The exact nature of this group is defined by the scope and headers attributes.
    th HtmlTableCellElement [headers, scope] true,
    /// The `<thead>` HTML element defines a set of rows defining the head of the columns of the table.
    thead HtmlTableSectionElement [] true,
    /// The `<time>` HTML element represents a specific period in time. It may include the datetime attribute to translate dates into machine-readable format, allowing for better search engine results or custom features such as reminders.
    time HtmlTimeElement [] true,
    ///	The `<title>` HTML element defines the document's title that is shown in a Browser's title bar or a page's tab. It only contains text; tags within the element are ignored.
    title HtmlTitleElement [] true,
    /// The `<tr>` HTML element defines a row of cells in a table. The row's cells can then be established using a mix of td (data cell) and th (header cell) elements.
    tr HtmlTableRowElement [] true,

    /// The `<ul>` HTML element represents an unordered list of items, typically rendered as a bulleted list.
    ul HtmlUListElement [] true,

}
