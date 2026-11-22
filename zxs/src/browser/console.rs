//! The original folder view: navigation, validation and presentation in Rust.
use super::{data::{self, Entry, Kind, Page}, host::{self, Element, Fault, Post, Result},
    labels::{self, Editor, Outcome}, route::{Folder, Id, Location, Panel, ROOTS}};
use crate::label_text;
use std::cell::{Cell, OnceCell, RefCell};

const SKIP: &str = "#zxs-main-content";

struct Share {
    id: Id,
    label: String,
}

struct View {
    kind: Panel,
    panel: Element,
    heading: Element,
    link: Element,
}

struct Pending {
    ticket: u64,
    folder: Folder,
    hash: String,
}

struct Shown {
    folder: Folder,
    entries: Vec<Entry>,
    scope: [u8; 32],
    writable: bool,
}

struct Console {
    views: [View; 4],
    secondary: Element,
    content: Element,
    corner_paint: Element,
    shares: Vec<Share>,
    next_share: Option<Id>,
    generation: Cell<u64>,
    pending: RefCell<Option<Pending>>,
    shown: RefCell<Option<Shown>>,
    csrf: String,
    editor: Editor,
}

thread_local! {
    static OWNER: OnceCell<Console> = const { OnceCell::new() };
}

#[unsafe(no_mangle)]
pub extern "C" fn zxs_mount() -> u32 {
    OWNER.with(|slot| {
        let mount = || {
            if slot.get().is_some() { return Err(Fault); }
            slot.set(Console::prepare()?).map_err(|_| Fault)?;
            slot.get().ok_or(Fault)?.navigate(false)
        };
        u32::from(mount().is_ok())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn zxs_hash_changed() {
    OWNER.with(|slot| {
        if let Some(console) = slot.get() && console.navigate(true).is_err() {
            let _ = console.message("目录暂时无法显示。", true);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn zxs_directory_result(ticket: u64, status: u32) {
    OWNER.with(|slot| {
        if let Some(console) = slot.get() {
            console.directory_result(ticket, status);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn zxs_folder_form(handle: u32) {
    OWNER.with(|slot| {
        if let Ok(form) = Element::take(handle)
            && let Some(console) = slot.get()
            && console.form(&form).is_err() {
            let _ = console.message("无法处理此标签操作。", true);
        }
    });
}

impl View {
    fn new(kind: Panel, link: Element) -> Result<Self> {
        let panel = Element::by_id(kind.id())?;
        let id = panel.attr("aria-labelledby")?.ok_or(Fault)?;
        let heading = Element::by_id(&id)?;
        Ok(Self { kind, panel, heading, link })
    }

    fn native(slot: &Element, kind: Panel) -> Result<Self> {
        let selector = format!(r##"a[href="#{}"]"##, kind.id());
        let link = slot.query(&selector)?;
        Self::new(kind, link)
    }

    fn show(&self, visible: bool, selected: bool) -> Result<()> {
        visibility(&self.panel, visible)?;
        if selected {
            self.link.set_attribute("aria-current", "page")
        } else {
            self.link.remove_attribute("aria-current")
        }
    }
}

impl Console {
    fn prepare() -> Result<Self> {
        let content = Element::by_id("zxs-folder-content")?;
        let corner_paint = Element::by_id("zxs-file-corner-source")?.first()?;
        let slot = Element::by_id("zxs-console-nav")?;
        let secondary = Element::by_id("zxs-console-secondary")?;
        let roots = Element::by_id("zxs-browser-roots")?;
        let csrf = roots.attr("data-zxs-csrf")?.filter(|value| label_text::hex::<32>(value).is_some()).ok_or(Fault)?;
        let count = roots.shares()?;
        if count > 64 { return Err(Fault); }
        let mut shares = Vec::new();
        for i in 0..count {
            let node = roots.share_at(i)?;
            let id = Id::parse(&node.attr("data-zxs-share")?.ok_or(Fault)?).ok_or(Fault)?;
            let label = node.text()?.filter(|s| !s.is_empty()).ok_or(Fault)?;
            if shares.iter().any(|row: &Share| row.id == id) {
                return Err(Fault);
            }
            shares.push(Share { id, label });
        }
        let next_share = roots.attr("data-zxs-next")?
            .map(|value| Id::parse(&value).ok_or(Fault)).transpose()?;
        let directory_link = link(ROOTS, "文件")?;
        directory_link.set_class_name("zxs-button")?;
        let views = [
            View::new(Panel::Folders, directory_link.clone())?,
            View::native(&slot, Panel::Shares)?,
            View::native(&slot, Panel::Versions)?,
            View::new(Panel::Account, Element::by_id("zxs-account-link")?)?,
        ];
        slot.prepend(&directory_link)?;
        Ok(Self {views, secondary, content, corner_paint, shares, next_share,
            generation: Cell::new(0), pending: RefCell::new(None), shown: RefCell::new(None),
            csrf, editor: Editor::new()? })
    }

    fn select(&self, location: &Location, focus: bool) -> Result<()> {
        let selected = location.selected();
        for view in &self.views {
            let active = selected == Some(view.kind);
            let visible = match selected {
                Some(_) => active,
                None => view.kind != Panel::Folders || *location == Location::Landing,
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

    fn navigate(&self, focus: bool) -> Result<()> {
        let hash = host::hash()?;
        if hash == SKIP {
            return Ok(());
        }
        let location = Location::parse(&hash);
        let ticket = self.ticket();
        self.pending.borrow_mut().take();
        self.shown.borrow_mut().take();
        self.editor.close()?;
        host::cancel_request();
        self.content.set_text("")?;
        self.select(&location, focus)?;
        self.content.remove_attribute("aria-busy")?;
        match location {
            Location::Landing | Location::Panel(Panel::Folders) => self.show_roots(),
            Location::InvalidDirectory => self.directory_error("目录链接无效。"),
            Location::Directory(folder) => {
                self.content.set_attribute("aria-busy", "true")?;
                self.message("正在加载…", false)?;
                if let Some(form) = folder.form(&self.csrf) {
                    host::post(ticket, Post::Filter, &form);
                } else {
                    host::request(ticket, &folder.request_path());
                }
                *self.pending.borrow_mut() = Some(Pending { ticket, folder, hash });
                Ok(())
            }
            Location::Panel(_) | Location::Other => Ok(()),
        }
    }

    fn ticket(&self) -> u64 {
        let ticket = self.generation.get().wrapping_add(1);
        self.generation.set(ticket);
        ticket
    }

    fn matches_hash(hash: &str) -> bool {
        matches!(host::hash().as_deref(), Ok(current) if current == hash || current == SKIP)
    }

    fn set_folder(&self, folder: &Folder) -> Result<()> {
        let hash = folder.fragment();
        if host::hash()? == hash { self.navigate(false) } else { host::set_hash(&hash) }
    }

    fn form(&self, form: &Element) -> Result<()> {
        let action = labels::action(form)?;
        let shown_guard = self.shown.borrow();
        let Some(shown) = shown_guard.as_ref() else { return Ok(()) };
        if !Self::matches_hash(&shown.folder.fragment()) { return Ok(()); }
        match action.as_str() {
            "filter" => {
                if let Some(next) = labels::filtered(form, &shown.folder)? {
                    drop(shown_guard);
                    // set_folder may directly restart an already-current first page.
                    return self.set_folder(&next);
                }
                Ok(())
            }
            "open" => {
                let file = labels::chosen(form)?;
                let row = shown.entries.iter().find(|row| row.id == file && row.kind == Kind::File).ok_or(Fault)?;
                self.editor.open(labels::Selection { folder: shown.folder.clone(), file,
                    position: row.position.ok_or(Fault)?, scope: shown.scope, writable: shown.writable,
                    name: row.name.clone() }, self.ticket(), &self.csrf)
            }
            "refresh" => {
                let at = shown.folder.destination(shown.folder.parent);
                drop(shown_guard);
                self.set_folder(&at)
            }
            other => self.editor.submit(other, form, self.ticket(), &self.csrf),
        }
    }

    fn label_result(&self, ticket: u64, status: u32) -> Result<()> {
        let visible = self.shown.borrow().as_ref().is_some_and(|page| Self::matches_hash(&page.folder.fragment()));
        if !visible { return Ok(()); }
        match self.editor.completion(ticket, status, self.ticket(), &self.csrf)? {
            Outcome::NoChange => Ok(()),
            Outcome::Reload(folder) => self.set_folder(&folder),
            Outcome::SignIn => host::sign_in(),
        }
    }

    fn directory_result(&self, ticket: u64, status: u32) {
        if self.pending.borrow().as_ref().is_none_or(|pending| pending.ticket != ticket) {
            if self.label_result(ticket, status).is_err() {
                let _ = self.message("暂时无法显示标签。", true);
            }
            return;
        }
        let Some(pending) = self.pending.borrow_mut().take() else { return };
        if !Self::matches_hash(&pending.hash) {
            return;
        }
        let render = || {
            self.content.remove_attribute("aria-busy")?;
            self.content.set_text("")?;
            match status {
                200 => match data::decode(&pending.folder) {
                    Some(page) => self.show_folder(pending.folder, page),
                    None => self.directory_error("读取失败，请稍后重试。"),
                },
                401 => host::sign_in(),
                403 | 404 | 409 => self.directory_error("无法打开此目录。"),
                _ => self.directory_error("读取失败，请稍后重试。"),
            }
        };
        if render().is_err() {
            let _ = self.content.set_text("");
            let _ = self.message("目录暂时无法显示。", true);
        }
    }

    fn share_name(&self, id: Id) -> &str {
        self.shares.iter().find(|s| s.id == id).map_or("共享目录", |s| s.label.as_str())
    }

    fn show_roots(&self) -> Result<()> {
        if self.shares.is_empty() {
            self.message("暂无可浏览的共享目录。", false)?;
        } else {
            let list = Self::list()?;
            for share in &self.shares {
                let folder = Folder { share: share.id, parent: None, page: None, filter: None };
                Self::entry(&list, &share.label, Kind::Directory, &folder.fragment())?;
            }
            self.show_rows(&list, false)?;
        }
        if let Some(next) = self.next_share {
            let a = Self::button(&format!("/?after={next}{ROOTS}"), "下一页共享")?;
            self.content.append_child(&a)?;
        }
        Ok(())
    }

    fn show_folder(&self, folder: Folder, page: Page) -> Result<()> {
        let crumb = self.breadcrumb(&folder, &page)?;
        self.content.append_child(&crumb)?;
        self.content.append_child(&labels::filter(&folder)?)?;
        self.editor.attach(&self.content)?;
        if page.entries.is_empty() {
            let message = if folder.page.is_some() { "此页没有更多项目。" }
                else if folder.filter.is_some() { "这个目录没有匹配标签的文件或子目录。" }
                else { "这个文件夹是空的。" };
            self.message(message, false)?;
        } else {
            let list = Self::list()?;
            let actions = page.entries.iter().any(|row| row.kind == Kind::File);
            for row in &page.entries {
                let href = match row.kind {
                    Kind::Directory => folder.destination(Some(row.id)).fragment(),
                    Kind::File => format!("/versions/history?file={}", row.id),
                };
                let line = Self::entry(&list, &row.name, row.kind, &href)?;
                if row.kind == Kind::File {
                    let button = Self::button(&format!("/versions?file={}", row.id), "手动提交")?;
                    button.set_attribute("aria-label", &format!("手动提交 {} 的版本", row.name))?;
                    let operations = node("div", "zxs-file-actions")?;
                    operations.append_child(&button)?;
                    operations.append_child(&labels::open_button(row.id, &row.name)?)?;
                    line.append_child(&operations)?;
                }
            }
            self.show_rows(&list, actions)?;
        }
        if folder.page.is_some() || page.next.is_some() {
            let pager = node("nav", "zxs-file-pagination")?;
            pager.set_attribute("aria-label", "目录分页")?;
            if folder.page.is_some() {
                let first = Self::button(&folder.destination(folder.parent).fragment(), "第一页")?;
                pager.append_child(&first)?;
            }
            if let Some(page) = page.next {
                let next = Self::button(&Folder {page: Some(page), ..folder.clone()}.fragment(), "下一页")?;
                pager.append_child(&next)?;
            }
            self.content.append_child(&pager)?;
        }
        *self.shown.borrow_mut() = Some(Shown { folder, entries: page.entries, scope: page.scope, writable: page.writable });
        Ok(())
    }

    fn breadcrumb(&self, folder: &Folder, page: &Page) -> Result<Element> {
        let nav = node("nav", "zxs-breadcrumb")?;
        nav.set_attribute("aria-label", "当前位置")?;
        let items = Element::new("ol", false)?;
        let root = link(ROOTS, "目录")?;
        let glyph = icon(HOME, "zxs-file-icon")?;
        root.prepend(&glyph)?;
        Self::crumb(&items, &root, false)?;
        let share = self.share_name(folder.share);
        let share_at = if folder.parent.is_none() {
            text("bdi", share)?
        } else {
            link(&folder.destination(None).fragment(), share)?
        };
        Self::crumb(&items, &share_at, folder.parent.is_none())?;
        for ancestor in &page.ancestors {
            let current = Some(ancestor.id) == folder.parent;
            let at = if current {
                text("bdi", &ancestor.name)?
            } else {
                link(&folder.destination(Some(ancestor.id)).fragment(), &ancestor.name)?
            };
            Self::crumb(&items, &at, current)?;
        }
        nav.append_child(&items)?;
        Ok(nav)
    }

    fn crumb(items: &Element, at: &Element, current: bool) -> Result<()> {
        let item = Element::new("li", false)?;
        if current {
            item.set_attribute("aria-current", "location")?;
        }
        item.append_child(at)?;
        items.append_child(&item)?;
        Ok(())
    }

    fn show_rows(&self, rows: &Element, actions: bool) -> Result<()> {
        let panel = node("div", "zxs-file-list-panel zxs-g4-surface")?;
        panel.append_child(&self.corner_paint)?;
        let heading = node("div", "zxs-file-row zxs-file-list-heading")?;
        heading.set_attribute("aria-hidden", "true")?;
        for title in ["名称", "类型", if actions { "操作" } else { "" }] {
            let column = text("span", title)?;
            heading.append_child(&column)?;
        }
        panel.append_child(&heading)?;
        panel.append_child(rows)?;
        self.content.append_child(&panel)?;
        Ok(())
    }

    fn list() -> Result<Element> {
        let list = node("ul", "zxs-file-list")?;
        list.set_attribute("role", "list")?;
        Ok(list)
    }

    fn entry(list: &Element, name: &str, kind: Kind, href: &str) -> Result<Element> {
        let row = node("li", "zxs-file-row")?;
        let a = node("a", "zxs-file-name")?;
        a.set_attribute("href", href)?;
        let glyph = match kind {
            Kind::Directory => icon(FOLDER, "zxs-file-icon zxs-file-icon--folder")?,
            Kind::File => icon(FILE, "zxs-file-icon zxs-file-icon--file")?,
        };
        a.append_child(&glyph)?;
        let name = text("bdi", name)?;
        a.append_child(&name)?;
        row.append_child(&a)?;
        let category = text("span", match kind {
            Kind::Directory => "目录",
            Kind::File => "文件",
        })?;
        category.set_class_name("zxs-file-type")?;
        row.append_child(&category)?;
        list.append_child(&row)?;
        Ok(row)
    }

    fn button(href: &str, label: &str) -> Result<Element> {
        let a = link(href, label)?;
        a.set_class_name("zxs-button")?;
        Ok(a)
    }

    fn directory_error(&self, note: &str) -> Result<()> {
        self.message(note, true)?;
        let home = Self::button(ROOTS, "返回目录")?;
        self.content.append_child(&home)?;
        Ok(())
    }

    fn message(&self, message: &str, alert: bool) -> Result<()> {
        let p = text("p", message)?;
        p.set_class_name("zxs-file-empty")?;
        p.set_attribute("role", if alert { "alert" } else { "status" })?;
        self.content.append_child(&p)?;
        Ok(())
    }
}

fn visibility(element: &Element, shown: bool) -> Result<()> {
    if shown {
        element.remove_attribute("hidden")
    } else {
        element.set_attribute("hidden", "")
    }
}

pub(super) fn node(tag: &str, class: &str) -> Result<Element> {
    let element = Element::new(tag, false)?;
    element.set_class_name(class)?;
    Ok(element)
}

pub(super) fn text(tag: &str, value: &str) -> Result<Element> {
    let element = Element::new(tag, false)?;
    element.set_text(value)?;
    Ok(element)
}

pub(super) fn link(href: &str, label: &str) -> Result<Element> {
    let link = text("a", label)?;
    link.set_attribute("href", href)?;
    Ok(link)
}

const FOLDER: &str = "M3 20V5h6l3 3h8v3M3 20h16l3-9H7l-4 9";
const FILE: &str = "M6 3h8l4 4v14H6V3m8 0v5h4M9 12h6m-6 4h6";
const HOME: &str = "m3 10 9-8 9 8M5 8v13h14V8M9 21v-8h6v8";

fn icon(path: &str, class: &str) -> Result<Element> {
    let svg = Element::new("svg", true)?;
    for (name, value) in [
        ("class", class), ("viewBox", "0 0 24 24"), ("width", "1em"), ("height", "1em"),
        ("stroke", "currentColor"), ("stroke-width", "1.7"), ("fill", "none"),
        ("aria-hidden", "true"), ("focusable", "false"),
    ] {
        svg.set_attribute(name, value)?;
    }
    let shape = Element::new("path", true)?;
    shape.set_attribute("d", path)?;
    svg.append_child(&shape)?;
    Ok(svg)
}
