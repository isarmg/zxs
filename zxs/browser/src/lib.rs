mod data;
mod route;

use data::{Failure, Kind, Page};
use js_sys::futures::spawn_local;
use route::{Folder, Id, Location, Panel, ROOTS};
use std::{cell::Cell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use web_sys::{Document, Element, Event, HtmlElement, Window};

const SKIP: &str = "#zxs-main-content";

struct Share {
    id: Id,
    label: String,
}

struct View {
    kind: Panel,
    panel: Element,
    heading: HtmlElement,
    link: Element,
}

struct Console {
    window: Window,
    document: Document,
    views: [View; 4],
    secondary: Element,
    content: Element,
    corner_paint: Element,
    shares: Vec<Share>,
    next_share: Option<Id>,
    generation: Cell<u64>,
}

#[wasm_bindgen(start)]
pub fn mount() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(missing)?;
    let document = window.document().ok_or_else(missing)?;
    let content = element(&document, "zxs-folder-content")?;
    let corner_paint = element(&document, "zxs-file-corner-source")?
        .first_element_child().ok_or_else(missing)?;
    let slot = element(&document, "zxs-console-nav")?;
    let secondary = element(&document, "zxs-console-secondary")?;
    let roots = element(&document, "zxs-browser-roots")?;
    let mut shares = Vec::new();
    let nodes = roots.query_selector_all("[data-zxs-share]")?;
    if nodes.length() > 64 {
        return Err(missing());
    }
    for i in 0..nodes.length() {
        let node = nodes.item(i).ok_or_else(missing)?.dyn_into::<Element>()?;
        let id = Id::parse(&node.get_attribute("data-zxs-share").ok_or_else(missing)?)
            .ok_or_else(missing)?;
        let label = node.text_content().filter(|s| !s.is_empty() && s.len() <= 255)
            .ok_or_else(missing)?;
        if shares.iter().any(|row: &Share| row.id == id) {
            return Err(missing());
        }
        shares.push(Share { id, label });
    }
    let next_share = roots.get_attribute("data-zxs-next")
        .map(|value| Id::parse(&value).ok_or_else(missing))
        .transpose()?;
    let directory_link = link(&document, ROOTS, "文件")?;
    directory_link.set_class_name("zxs-button");
    let views = [
        View::new(&document, Panel::Folders, directory_link.clone())?,
        View::native(&document, &slot, Panel::Shares)?,
        View::native(&document, &slot, Panel::Versions)?,
        View::new(&document, Panel::Account, element(&document, "zxs-account-link")?)?,
    ];
    let console = Rc::new(Console {
        window, document, views, secondary, content, corner_paint,
        shares, next_share, generation: Cell::new(0),
    });
    let owner = Rc::clone(&console);
    let on_hash = Closure::<dyn FnMut(Event)>::new(move |_| {
        if owner.navigate(true).is_err() {
            let _ = owner.message("目录暂时无法显示。", true);
        }
    });
    console.window.add_event_listener_with_callback("hashchange", on_hash.as_ref().unchecked_ref())?;
    on_hash.forget();
    slot.insert_before(&directory_link, slot.first_child().as_ref())?;
    console.navigate(false)
}

impl View {
    fn new(document: &Document, kind: Panel, link: Element) -> Result<Self, JsValue> {
        let panel = element(document, kind.id())?;
        let id = panel.get_attribute("aria-labelledby").ok_or_else(missing)?;
        let heading = element(document, &id)?.dyn_into::<HtmlElement>()?;
        Ok(Self { kind, panel, heading, link })
    }

    fn native(document: &Document, slot: &Element, kind: Panel) -> Result<Self, JsValue> {
        let selector = format!(r##"a[href="#{}"]"##, kind.id());
        let link = slot.query_selector(&selector)?.ok_or_else(missing)?;
        Self::new(document, kind, link)
    }

    fn show(&self, visible: bool, selected: bool) -> Result<(), JsValue> {
        visibility(&self.panel, visible)?;
        if selected {
            self.link.set_attribute("aria-current", "page")
        } else {
            self.link.remove_attribute("aria-current")
        }
    }
}

impl Console {
    fn select(&self, location: Location, focus: bool) -> Result<(), JsValue> {
        let selected = location.selected();
        for view in &self.views {
            let active = selected == Some(view.kind);
            let visible = match selected {
                Some(_) => active,
                None => view.kind != Panel::Folders || location == Location::Landing,
            };
            view.show(visible, active)?;
        }
        visibility(&self.secondary, matches!(selected, Some(Panel::Versions | Panel::Account)) || selected.is_none())?;
        if focus && let Some(view) = self.views.iter().find(|v| selected == Some(v.kind)) {
            view.heading.set_attribute("tabindex", "-1")?;
            view.heading.focus()?;
        }
        Ok(())
    }

    fn navigate(self: &Rc<Self>, focus: bool) -> Result<(), JsValue> {
        let hash = self.window.location().hash()?;
        if hash == SKIP {
            return Ok(());
        }
        let location = Location::parse(&hash);
        let ticket = self.generation.get().wrapping_add(1);
        self.generation.set(ticket);
        self.content.set_text_content(None);
        self.select(location, focus)?;
        let listing = self.content.clone();
        listing.remove_attribute("aria-busy")?;
        match location {
            Location::Landing | Location::Panel(Panel::Folders) => self.show_roots(),
            Location::InvalidDirectory => self.directory_error("目录链接无效。"),
            Location::Directory(folder) => {
                listing.set_attribute("aria-busy", "true")?;
                self.message("正在加载…", false)?;
                let owner = Rc::clone(self);
                spawn_local(async move {
                    let result = data::read(&owner.window, folder).await;
                    let selected = owner.window.location().hash().ok();
                    if owner.generation.get() != ticket
                        || !matches!(selected.as_deref(), Some(current) if current == hash || current == SKIP)
                    {
                        return;
                    }
                    let _ = listing.remove_attribute("aria-busy");
                    owner.content.set_text_content(None);
                    let rendered = match result {
                        Ok(page) => owner.show_folder(folder, page),
                        Err(Failure::Expired) => owner.window.location().assign("/login"),
                        Err(Failure::Changed) => owner.directory_error("无法打开此目录。"),
                        Err(Failure::Unavailable) => owner.directory_error("读取失败，请稍后重试。"),
                    };
                    if rendered.is_err() {
                        owner.content.set_text_content(None);
                        let _ = owner.message("目录暂时无法显示。", true);
                    }
                });
                Ok(())
            }
            Location::Panel(_) | Location::Other => Ok(()),
        }
    }

    fn share_name(&self, id: Id) -> &str {
        self.shares.iter().find(|s| s.id == id).map(|s| s.label.as_str()).unwrap_or("共享目录")
    }

    fn show_roots(&self) -> Result<(), JsValue> {
        if self.shares.is_empty() {
            self.message("暂无可浏览的共享目录。", false)?;
        } else {
            let list = self.list()?;
            for share in &self.shares {
                let folder = Folder { share: share.id, parent: None, page: None };
                self.entry(&list, &share.label, Kind::Directory, &folder.fragment())?;
            }
            self.show_rows(&list, false)?;
        }
        if let Some(next) = self.next_share {
            let a = self.button(&format!("/?after={next}{ROOTS}"), "下一页共享")?;
            self.content.append_child(&a)?;
        }
        Ok(())
    }

    fn show_folder(&self, folder: Folder, page: Page) -> Result<(), JsValue> {
        let crumb = self.breadcrumb(folder, &page)?;
        self.content.append_child(&crumb)?;
        if page.entries.is_empty() {
            let message = if folder.page.is_some() { "此页没有更多项目。" } else { "这个文件夹是空的。" };
            self.message(message, false)?;
        } else {
            let list = self.list()?;
            let actions = page.entries.iter().any(|row| row.kind == Kind::File);
            for row in page.entries {
                let href = match row.kind {
                    Kind::Directory => folder.destination(Some(row.id)).fragment(),
                    Kind::File => format!("/versions/history?file={}", row.id),
                };
                let line = self.entry(&list, &row.name, row.kind, &href)?;
                if row.kind == Kind::File {
                    let button = self.button(&format!("/versions?file={}", row.id), "手动提交")?;
                    button.set_attribute("aria-label", &format!("手动提交 {} 的版本", row.name))?;
                    line.append_child(&button)?;
                }
            }
            self.show_rows(&list, actions)?;
        }
        if folder.page.is_some() || page.next.is_some() {
            let pager = node(&self.document, "nav", "zxs-file-pagination")?;
            pager.set_attribute("aria-label", "目录分页")?;
            if folder.page.is_some() {
                let first = self.button(&folder.destination(folder.parent).fragment(), "第一页")?;
                pager.append_child(&first)?;
            }
            if let Some(page) = page.next {
                let next = self.button(&Folder {page: Some(page), ..folder}.fragment(), "下一页")?;
                pager.append_child(&next)?;
            }
            self.content.append_child(&pager)?;
        }
        Ok(())
    }

    fn breadcrumb(&self, folder: Folder, page: &Page) -> Result<Element, JsValue> {
        let nav = node(&self.document, "nav", "zxs-breadcrumb")?;
        nav.set_attribute("aria-label", "当前位置")?;
        let items = self.document.create_element("ol")?;
        let root = link(&self.document, ROOTS, "目录")?;
        let glyph = icon(&self.document, HOME, "zxs-file-icon")?;
        root.insert_before(&glyph, root.first_child().as_ref())?;
        self.crumb(&items, root, false)?;
        let share = self.share_name(folder.share);
        let share_at = if folder.parent.is_none() {
            text(&self.document, "bdi", share)?
        } else {
            link(&self.document, &folder.destination(None).fragment(), share)?
        };
        self.crumb(&items, share_at, folder.parent.is_none())?;
        for ancestor in &page.ancestors {
            let current = Some(ancestor.id) == folder.parent;
            let at = if current {
                text(&self.document, "bdi", &ancestor.name)?
            } else {
                link(&self.document, &folder.destination(Some(ancestor.id)).fragment(), &ancestor.name)?
            };
            self.crumb(&items, at, current)?;
        }
        nav.append_child(&items)?;
        Ok(nav)
    }

    fn crumb(&self, items: &Element, at: Element, current: bool) -> Result<(), JsValue> {
        let item = self.document.create_element("li")?;
        if current {
            item.set_attribute("aria-current", "location")?;
        }
        item.append_child(&at)?;
        items.append_child(&item)?;
        Ok(())
    }

    fn show_rows(&self, rows: &Element, actions: bool) -> Result<(), JsValue> {
        let panel = node(&self.document, "div", "zxs-file-list-panel zxs-g4-surface")?;
        panel.append_child(&self.corner_paint)?;
        let heading = node(&self.document, "div", "zxs-file-row zxs-file-list-heading")?;
        heading.set_attribute("aria-hidden", "true")?;
        for title in ["名称", "类型", if actions { "操作" } else { "" }] {
            let column = text(&self.document, "span", title)?;
            heading.append_child(&column)?;
        }
        panel.append_child(&heading)?;
        panel.append_child(rows)?;
        self.content.append_child(&panel)?;
        Ok(())
    }

    fn list(&self) -> Result<Element, JsValue> {
        let list = node(&self.document, "ul", "zxs-file-list")?;
        list.set_attribute("role", "list")?;
        Ok(list)
    }

    fn entry(&self, list: &Element, name: &str, kind: Kind, href: &str) -> Result<Element, JsValue> {
        let row = node(&self.document, "li", "zxs-file-row")?;
        let a = node(&self.document, "a", "zxs-file-name")?;
        a.set_attribute("href", href)?;
        let glyph = match kind {
            Kind::Directory => icon(&self.document, FOLDER, "zxs-file-icon zxs-file-icon--folder")?,
            Kind::File => icon(&self.document, FILE, "zxs-file-icon zxs-file-icon--file")?,
        };
        a.append_child(&glyph)?;
        let name = text(&self.document, "bdi", name)?;
        a.append_child(&name)?;
        row.append_child(&a)?;
        let category = text(&self.document, "span", match kind {
            Kind::Directory => "目录",
            Kind::File => "文件",
        })?;
        category.set_class_name("zxs-file-type");
        row.append_child(&category)?;
        list.append_child(&row)?;
        Ok(row)
    }

    fn button(&self, href: &str, label: &str) -> Result<Element, JsValue> {
        let a = link(&self.document, href, label)?;
        a.set_class_name("zxs-button");
        Ok(a)
    }

    fn directory_error(&self, note: &str) -> Result<(), JsValue> {
        self.message(note, true)?;
        let home = self.button(ROOTS, "返回目录")?;
        self.content.append_child(&home)?;
        Ok(())
    }

    fn message(&self, message: &str, alert: bool) -> Result<(), JsValue> {
        let p = text(&self.document, "p", message)?;
        p.set_class_name("zxs-file-empty");
        p.set_attribute("role", if alert { "alert" } else { "status" })?;
        self.content.append_child(&p)?;
        Ok(())
    }
}

fn visibility(element: &Element, shown: bool) -> Result<(), JsValue> {
    if shown {
        element.remove_attribute("hidden")
    } else {
        element.set_attribute("hidden", "")
    }
}

fn missing() -> JsValue {
    JsValue::from_str("ZXS browser mount is incomplete")
}

fn element(document: &Document, id: &str) -> Result<Element, JsValue> {
    document.get_element_by_id(id).ok_or_else(missing)
}

fn node(document: &Document, tag: &str, class: &str) -> Result<Element, JsValue> {
    let element = document.create_element(tag)?;
    element.set_class_name(class);
    Ok(element)
}

fn text(document: &Document, tag: &str, value: &str) -> Result<Element, JsValue> {
    let element = document.create_element(tag)?;
    element.set_text_content(Some(value));
    Ok(element)
}

fn link(document: &Document, href: &str, label: &str) -> Result<Element, JsValue> {
    let link = text(document, "a", label)?;
    link.set_attribute("href", href)?;
    Ok(link)
}

const FOLDER: &str = "M3 20V5h6l3 3h8v3M3 20h16l3-9H7l-4 9";
const FILE: &str = "M6 3h8l4 4v14H6V3m8 0v5h4M9 12h6m-6 4h6";
const HOME: &str = "m3 10 9-8 9 8M5 8v13h14V8M9 21v-8h6v8";

fn icon(document: &Document, path: &str, class: &str) -> Result<Element, JsValue> {
    const SVG: Option<&str> = Some("http://www.w3.org/2000/svg");
    let svg = document.create_element_ns(SVG, "svg")?;
    for (name, value) in [
        ("class", class), ("viewBox", "0 0 24 24"), ("width", "1em"), ("height", "1em"),
        ("stroke", "currentColor"), ("stroke-width", "1.7"), ("fill", "none"),
        ("aria-hidden", "true"), ("focusable", "false"),
    ] {
        svg.set_attribute(name, value)?;
    }
    let shape = document.create_element_ns(SVG, "path")?;
    shape.set_attribute("d", path)?;
    svg.append_child(&shape)?;
    Ok(svg)
}
