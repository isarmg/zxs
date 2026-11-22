//! Directory label filter and the original row's label editor.
//! DOM inputs carry only text or a reference to a currently rendered row.
use super::{console::{link, node, text}, data, host::{self, Element, Fault, Post, Result}, route::{Folder, Id}};
use crate::label_text::{self, Label, MAX};
use std::cell::RefCell;

const ACTION: &str = "data-zxs-action";
const VALUE: &str = "data-zxs-value";
const BAD_LABEL: &str = "请输入非空单行标签，最多 255 个 UTF-8 字节。";

pub(super) fn action(form: &Element) -> Result<String> {
    form.attr(ACTION)?.ok_or(Fault)
}

fn command(action: &str, caption: &str, value: Option<&str>) -> Result<Element> {
    let form = Element::new("form", false)?;
    form.set_attribute(ACTION, action)?;
    if let Some(value) = value { form.set_attribute(VALUE, value)?; }
    let button = node("button", "zxs-button")?;
    button.set_attribute("type", "submit")?;
    button.set_text(caption)?;
    form.append_child(&button)?;
    Ok(form)
}

fn label_form(action: &str, caption: &str, placeholder: &str, value: &str) -> Result<Element> {
    let form = node("form", "zxs-label-form")?;
    form.set_attribute(ACTION, action)?;
    let input = node("input", "zxs-input")?;
    for (name, value) in [("type", "text"), ("name", "label"), ("aria-label", placeholder),
        ("placeholder", placeholder), ("value", value), ("maxlength", "255"), ("required", "")] {
        input.set_attribute(name, value)?;
    }
    form.append_child(&input)?;
    let button = node("button", "zxs-button")?;
    button.set_attribute("type", "submit")?;
    button.set_text(caption)?;
    form.append_child(&button)?;
    let note = text("span", "")?;
    note.set_attribute("role", "status")?;
    form.append_child(&note)?;
    Ok(form)
}

fn input(form: &Element) -> Result<Option<Label>> {
    let input = form.query("input[name=label]")?;
    let name = input.value::<MAX>().ok().and_then(|s| Label::parse(&s));
    form.query("[role=status]")?.set_text(if name.is_some() { "" } else { BAD_LABEL })?;
    Ok(name)
}

pub(super) fn filter(folder: &Folder) -> Result<Element> {
    let controls = node("div", "zxs-label-controls")?;
    let value = folder.filter.as_ref().map_or("", Label::text);
    controls.append_child(&label_form("filter", "按标签筛选", "精确标签", value)?)?;
    if folder.filter.is_some() {
        let clear = link(&folder.filtered(None).fragment(), "清除筛选")?;
        clear.set_class_name("zxs-button")?;
        controls.append_child(&clear)?;
    }
    Ok(controls)
}

pub(super) fn filtered(form: &Element, folder: &Folder) -> Result<Option<Folder>> {
    Ok(input(form)?.map(|name| folder.filtered(Some(name))))
}

pub(super) fn open_button(file: Id, caption: &str) -> Result<Element> {
    let form = command("open", "标签", Some(&file.to_string()))?;
    form.first()?.set_attribute("aria-label", &format!("管理 {caption} 的标签"))?;
    Ok(form)
}

pub(super) fn chosen(form: &Element) -> Result<Id> {
    Id::parse(&form.attr(VALUE)?.ok_or(Fault)?).ok_or(Fault)
}

pub(super) struct Selection {
    pub(super) folder: Folder,
    pub(super) file: Id,
    pub(super) position: Id,
    pub(super) scope: [u8; 32],
    pub(super) writable: bool,
    pub(super) name: String,
}

enum Job {
    List(Option<Label>),
    Set { label: Label, present: bool },
}

impl Job {
    fn form(&self, selected: &Selection, csrf: &str) -> (Post, String) {
        let parent = selected.folder.parent.map_or_else(|| "-".into(), |id| id.to_string());
        let scope = label_text::to_hex(&selected.scope);
        let (purpose, action, label) = match self {
            Self::List(after) => (Post::Labels, "list", after.as_ref().map_or_else(|| "-".into(), Label::encoded)),
            Self::Set { label, present } => (Post::Edit, if *present { "add" } else { "remove" }, label.encoded()),
        };
        (purpose, format!("csrf={csrf}&share={}&parent={parent}&file={}&position={}&scope={scope}&action={action}&label={label}",
            selected.folder.share, selected.file, selected.position))
    }
}

struct State {
    selected: Selection,
    pending: Option<(u64, Job)>,
    shown: Vec<Label>,
    next: Option<Label>,
}

pub(super) enum Outcome { NoChange, Reload(Folder), SignIn }

pub(super) struct Editor {
    section: Element,
    content: Element,
    heading: Element,
    state: RefCell<Option<State>>,
}

impl Editor {
    pub(super) fn new() -> Result<Self> {
        let section = node("section", "zxs-label-editor")?;
        section.set_attribute("hidden", "")?;
        section.set_attribute("aria-labelledby", "zxs-label-heading")?;
        let heading = text("h3", "文件标签")?;
        heading.set_attribute("id", "zxs-label-heading")?;
        heading.set_attribute("tabindex", "-1")?;
        section.append_child(&heading)?;
        let content = Element::new("div", false)?;
        section.append_child(&content)?;
        section.append_child(&command("close", "关闭标签", None)?)?;
        Ok(Self { section, content, heading, state: RefCell::new(None) })
    }

    pub(super) fn attach(&self, container: &Element) -> Result<()> {
        container.append_child(&self.section)
    }

    pub(super) fn close(&self) -> Result<()> {
        self.state.borrow_mut().take();
        self.heading.set_text("文件标签")?;
        self.content.set_text("")?;
        self.section.set_attribute("hidden", "")
    }

    pub(super) fn open(&self, selected: Selection, ticket: u64, csrf: &str) -> Result<()> {
        self.heading.set_text(&format!("文件标签：{}", selected.name))?;
        self.section.remove_attribute("hidden")?;
        *self.state.borrow_mut() = Some(State { selected, pending: None, shown: Vec::new(), next: None });
        self.request(Job::List(None), ticket, csrf)?;
        self.heading.focus()
    }

    fn request(&self, job: Job, ticket: u64, csrf: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let Some(state) = state.as_mut() else { return Ok(()) };
        if state.pending.is_some() { return Ok(()); }
        self.message(match &job {
            Job::List(_) => "正在读取标签…",
            Job::Set { .. } => "正在保存标签…",
        })?;
        let (purpose, body) = job.form(&state.selected, csrf);
        state.pending = Some((ticket, job));
        host::post(ticket, purpose, &body);
        Ok(())
    }

    pub(super) fn submit(&self, action: &str, form: &Element, ticket: u64, csrf: &str) -> Result<()> {
        if action == "close" { return self.close(); }
        let job = {
            let state = self.state.borrow();
            let Some(state) = state.as_ref() else { return Ok(()) };
            if state.pending.is_some() { return Ok(()); }
            match action {
                "first" => Job::List(None),
                "next" => Job::List(Some(state.next.clone().ok_or(Fault)?)),
                "add" if state.selected.writable => {
                    let Some(label) = input(form)? else { return Ok(()) };
                    Job::Set { label, present: true }
                }
                "remove" if state.selected.writable => {
                    let index = form.attr(VALUE)?.and_then(|value| value.parse::<usize>().ok()).ok_or(Fault)?;
                    let label = state.shown.get(index).cloned().ok_or(Fault)?;
                    Job::Set { label, present: false }
                }
                _ => return Ok(()),
            }
        };
        self.request(job, ticket, csrf)
    }

    pub(super) fn completion(&self, ticket: u64, status: u32, next_ticket: u64, csrf: &str) -> Result<Outcome> {
        let reread = {
            let mut state = self.state.borrow_mut();
            let Some(state) = state.as_mut() else { return Ok(Outcome::NoChange) };
            if state.pending.as_ref().is_none_or(|(id, _)| *id != ticket) {
                return Ok(Outcome::NoChange);
            }
            let Some((_, job)) = state.pending.take() else { return Ok(Outcome::NoChange) };
            state.next = None;
            state.shown.clear();
            match (status, job) {
                (401, _) => return Ok(Outcome::SignIn),
                (204, Job::Set { label, present }) => {
                    if !present && state.selected.folder.filter.as_ref() == Some(&label) {
                        let at = &state.selected.folder;
                        return Ok(Outcome::Reload(at.destination(at.parent)));
                    }
                    true // Read the confirmed state; never retry a mutation.
                }
                (200, Job::List(after)) => {
                    if let Some((names, next)) = data::labels(after.as_ref()) {
                        state.shown = names;
                        state.next = next;
                        self.show(state, after.is_some())?;
                    } else {
                        self.failed("暂时无法读取标签。", "first")?;
                    }
                    false
                }
                (403 | 409, _) => {
                    self.failed("文件或共享已变化，请重新打开目录。", "refresh")?;
                    false
                }
                (_, Job::Set { .. }) => {
                    self.failed("修改结果未确认，请重新查看标签。", "first")?;
                    false
                }
                _ => {
                    self.failed("暂时无法读取标签。", "first")?;
                    false
                }
            }
        };
        if reread { self.request(Job::List(None), next_ticket, csrf)?; }
        Ok(Outcome::NoChange)
    }

    fn message(&self, message: &str) -> Result<()> {
        self.content.set_text("")?;
        let note = text("p", message)?;
        note.set_attribute("role", "status")?;
        self.content.append_child(&note)
    }

    fn failed(&self, message: &str, action: &str) -> Result<()> {
        self.message(message)?;
        self.content.append_child(&command(action, if action == "refresh" { "刷新目录" } else { "重新查看标签" }, None)?)
    }

    fn show(&self, state: &State, later_page: bool) -> Result<()> {
        self.content.set_text("")?;
        if state.shown.is_empty() {
            self.message(if later_page { "此页没有更多标签。" } else { "没有标签。" })?;
        } else {
            let list = Element::new("ul", false)?;
            for (index, label) in state.shown.iter().enumerate() {
                let row = node("li", "zxs-label-row")?;
                row.append_child(&text("bdi", label.text())?)?;
                if state.selected.writable {
                    let remove = command("remove", "移除", Some(&index.to_string()))?;
                    remove.first()?.set_attribute("aria-label", &format!("移除标签 {}", label.text()))?;
                    row.append_child(&remove)?;
                }
                list.append_child(&row)?;
            }
            self.content.append_child(&list)?;
        }
        let tools = node("div", "zxs-label-controls")?;
        if later_page { tools.append_child(&command("first", "第一组标签", None)?)?; }
        if state.next.is_some() { tools.append_child(&command("next", "下一组标签", None)?)?; }
        if state.selected.writable {
            tools.append_child(&label_form("add", "添加标签", "新标签", "")?)?;
        }
        self.content.append_child(&tools)
    }
}
