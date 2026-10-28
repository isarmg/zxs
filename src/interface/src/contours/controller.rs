use super::{frame, geometry, membership, mutations, paint};
use frame::{FrameWork, Invalidation, Step, TransitionLists, TransitionScope, Trigger};
use mutations::OutputWrites;
use paint::{CssSnapshot, fmt, number, owned_paint_property};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{
    CssStyleDeclaration, Document, Element, Event, EventTarget, HtmlElement, MediaQueryList,
    MutationObserver, MutationObserverInit, Node, ResizeObserver, SvgsvgElement, Window,
};

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
const TRANSITIONS: [&str; 5] = [
    "transition-property",
    "transition-duration",
    "transition-delay",
    "transition-timing-function",
    "transition-behavior",
];
const INTERACTIONS: [&str; 15] = [
    "pointerover",
    "pointerout",
    "pointerdown",
    "pointerup",
    "focusin",
    "focusout",
    "keydown",
    "keyup",
    "change",
    "input",
    "load",
    "DOMContentLoaded",
    "scroll",
    "beforetoggle",
    "toggle",
];
thread_local! { static CONTROLLERS: RefCell<Vec<Weak<Inner>>> = const { RefCell::new(Vec::new()) }; }

/// A shared lifetime handle. Dropping the last handle restores all owned paint.
#[derive(Clone)]
pub struct ContourController(Rc<Inner>);
impl ContourController {
    pub fn refresh(&self) {
        self.0.schedule();
    }
    pub fn refresh_now(&self) -> Result<(), JsValue> {
        self.0.refresh_now()
    }
    pub fn stop(&self) {
        self.0.stop();
    }
    pub fn same_instance(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Start contours in a browser admitted by the application startup contract.
pub fn start(document: &Document) -> Result<ContourController, JsValue> {
    if let Some(previous) = CONTROLLERS.with(|controllers| {
        let mut controllers = controllers.borrow_mut();
        controllers.retain(|entry| entry.strong_count() > 0);
        controllers
            .iter()
            .filter_map(Weak::upgrade)
            .find(|entry| !entry.stopped.get() && entry.document == *document)
    }) {
        return Ok(ContourController(previous));
    }
    let view = document
        .default_view()
        .ok_or_else(|| JsValue::from_str("Contours require a live browser document"))?;
    register_properties(&view)?;
    let root = document
        .document_element()
        .ok_or_else(|| JsValue::from_str("Browser document root is missing"))?;
    let computed = view
        .get_computed_style(&root)?
        .ok_or_else(|| JsValue::from_str("Computed browser styles are unavailable"))?;
    let all_properties = (0..computed.length())
        .map(|index| computed.item(index))
        .filter(|name| !owned_paint_property(name))
        .collect();
    let draft = document
        .create_element("div")?
        .unchecked_into::<HtmlElement>()
        .style();
    let color_scheme = view
        .match_media("(prefers-color-scheme: dark)")?
        .ok_or_else(|| JsValue::from_str("Color-scheme media query is unavailable"))?;
    let inner = Rc::new(Inner {
        document: document.clone(),
        view,
        draft,
        all_properties,
        color_scheme,
        state: RefCell::new(State::default()),
        invalidation: RefCell::new(Invalidation::default()),
        membership: RefCell::new(membership::Membership::new(document)?),
        stopped: Cell::new(false),
        updating: Cell::new(false),
        deferred_dirty: Cell::new(false),
        deferred_refresh: Cell::new(false),
        deferred_stop: Cell::new(false),
        frame: Cell::new(None),
        setup: OnceCell::new(),
    });
    inner.install()?;
    CONTROLLERS.with(|controllers| controllers.borrow_mut().push(Rc::downgrade(&inner)));
    inner.refresh_now()?;
    Ok(ContourController(inner))
}

fn register_properties(view: &Window) -> Result<(), JsValue> {
    let css = js_sys::Reflect::get(view, &JsValue::from_str("CSS"))?;
    let register = js_sys::Reflect::get(&css, &JsValue::from_str("registerProperty"))?
        .unchecked_into::<js_sys::Function>();
    for (name, syntax, initial) in [
        ("--zxs-contour-extent", "<length-percentage>", "0px"),
        ("--zxs-contour-mode", "*", "paint"),
        ("--zxs-contour-foreground", "<number>", "0"),
    ] {
        let options = js_sys::Object::new();
        for (key, value) in [
            ("name", JsValue::from_str(name)),
            ("syntax", JsValue::from_str(syntax)),
            ("inherits", JsValue::FALSE),
            ("initialValue", JsValue::from_str(initial)),
        ] {
            js_sys::Reflect::set(&options, &JsValue::from_str(key), &value)?;
        }
        if let Err(error) = register.call1(&css, &options)
            && !(owner_instance(view, &error, "DOMException")
                && error.unchecked_ref::<web_sys::DomException>().name()
                    == "InvalidModificationError")
        {
            return Err(error);
        }
    }
    Ok(())
}

fn callable(value: JsValue) -> Result<js_sys::Function, JsValue> {
    if !value.is_function() {
        return Err(JsValue::from_str("Required browser method is unavailable"));
    }
    Ok(value.unchecked_into())
}

// Browser constructors belong to the document's realm. Global instanceof
// checks reject an otherwise valid iframe Element or DOMException.
fn owner_instance(view: &Window, value: &JsValue, name: &str) -> bool {
    let check = || -> Result<bool, JsValue> {
        let constructor = js_sys::Reflect::get(view, &JsValue::from_str(name))?;
        let has_instance = callable(js_sys::Reflect::get(
            &constructor,
            js_sys::Symbol::has_instance().as_ref(),
        )?)?;
        has_instance
            .call1(&constructor, value)?
            .as_bool()
            .ok_or_else(|| JsValue::from_str("Browser instanceof result is not boolean"))
    };
    match check() {
        Ok(matches) => matches,
        Err(error) => {
            web_sys::console::warn_2(
                &"Could not inspect the event's browser realm".into(),
                &error,
            );
            false
        }
    }
}

struct Inner {
    document: Document,
    view: Window,
    draft: CssStyleDeclaration,
    all_properties: Vec<String>,
    color_scheme: MediaQueryList,
    state: RefCell<State>,
    // Work accumulated by triggers and taken once per frame.
    invalidation: RefCell<Invalidation<Element>>,
    membership: RefCell<membership::Membership>,
    stopped: Cell<bool>,
    // Custom-element attribute callbacks can synchronously reenter public APIs
    // or dispatch events while paint owns the state borrow.
    updating: Cell<bool>,
    deferred_dirty: Cell<bool>,
    deferred_refresh: Cell<bool>,
    deferred_stop: Cell<bool>,
    frame: Cell<Option<i32>>,
    setup: OnceCell<Setup>,
}
struct Setup {
    observer: MutationObserver,
    discovery: MutationObserver,
    _discovery: Closure<dyn FnMut(js_sys::Array, MutationObserver)>,
    resize: ResizeObserver,
    _mutation: Closure<dyn FnMut(js_sys::Array, MutationObserver)>,
    _resize: Closure<dyn FnMut(js_sys::Array, ResizeObserver)>,
    animation_frame: Closure<dyn FnMut(f64)>,
    listeners: Vec<Listener>,
}
struct Listener {
    target: EventTarget,
    name: &'static str,
    capture: bool,
    callback: Closure<dyn FnMut(Event)>,
}
impl Listener {
    fn detach(&self) {
        if let Err(error) = self.target.remove_event_listener_with_callback_and_bool(
            self.name,
            self.callback.as_ref().unchecked_ref(),
            self.capture,
        ) {
            web_sys::console::warn_2(&"Could not detach a contour listener".into(), &error);
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.detach();
    }
}

#[derive(Clone, Copy)]
enum EventKind {
    Interaction,
    AnimationStart,
    AnimationEnd,
    Schedule,
    PageHide,
}
impl Inner {
    fn install(self: &Rc<Self>) -> Result<(), JsValue> {
        let weak = Rc::downgrade(self);
        let mutation = Closure::wrap(Box::new(move |_: js_sys::Array, _: MutationObserver| {
            if let Some(inner) = weak.upgrade() {
                inner.invalidate(Trigger::Author);
                inner.schedule();
            }
        })
            as Box<dyn FnMut(js_sys::Array, MutationObserver)>);
        let observer = MutationObserver::new(mutation.as_ref().unchecked_ref())?;
        let weak = Rc::downgrade(self);
        let discovery_callback = Closure::wrap(Box::new(
            move |records: js_sys::Array, _: MutationObserver| {
                if let Some(inner) = weak.upgrade()
                    && !inner.stopped.get()
                    && records.length() > 0
                {
                    inner
                        .membership
                        .borrow_mut()
                        .reconcile(&inner.document, &mutations::records(records));
                    inner.invalidate(Trigger::Structure);
                    inner.schedule();
                }
            },
        )
            as Box<dyn FnMut(js_sys::Array, MutationObserver)>);
        let discovery = MutationObserver::new(discovery_callback.as_ref().unchecked_ref())?;

        let weak = Rc::downgrade(self);
        let resize_callback = Closure::wrap(Box::new(move |_: js_sys::Array, _: ResizeObserver| {
            if let Some(inner) = weak.upgrade() {
                inner.invalidate(Trigger::Passive);
                inner.schedule();
            }
        })
            as Box<dyn FnMut(js_sys::Array, ResizeObserver)>);
        let resize = ResizeObserver::new(resize_callback.as_ref().unchecked_ref())?;
        let weak = Rc::downgrade(self);
        let animation_frame = Closure::wrap(Box::new(move |_: f64| {
            if let Some(inner) = weak.upgrade() {
                inner.frame.set(None);
                if let Err(error) = inner.update() {
                    web_sys::console::error_2(
                        &JsValue::from_str("Contour decoration refresh failed"),
                        &error,
                    );
                }
            }
        }) as Box<dyn FnMut(f64)>);
        let mut listeners = Vec::new();
        for name in INTERACTIONS {
            listeners.push(self.listen(
                self.document.as_ref(),
                name,
                true,
                EventKind::Interaction,
            )?);
        }
        for name in ["transitionrun", "animationstart"] {
            listeners.push(self.listen(
                self.document.as_ref(),
                name,
                true,
                EventKind::AnimationStart,
            )?);
        }
        for name in [
            "transitionend",
            "transitioncancel",
            "animationend",
            "animationcancel",
        ] {
            listeners.push(self.listen(
                self.document.as_ref(),
                name,
                true,
                EventKind::AnimationEnd,
            )?);
        }
        listeners.push(self.listen(
            self.color_scheme.as_ref(),
            "change",
            false,
            EventKind::Schedule,
        )?);
        listeners.push(self.listen(
            self.document.fonts().as_ref(),
            "loadingdone",
            false,
            EventKind::Schedule,
        )?);
        listeners.push(self.listen(self.view.as_ref(), "pagehide", false, EventKind::PageHide)?);
        assert!(
            self.setup
                .set(Setup {
                    observer,
                    discovery,
                    _discovery: discovery_callback,
                    resize,
                    _mutation: mutation,
                    _resize: resize_callback,
                    animation_frame,
                    listeners,
                })
                .is_ok(),
            "invariant: contour setup is installed once"
        );
        // Store ownership before observing: later installation failure then
        // disconnects the native observer through Inner::drop, not a dead closure.
        let discovery_options = MutationObserverInit::new();
        discovery_options.set_child_list(true);
        discovery_options.set_subtree(true);
        self.setup
            .get()
            .expect("invariant: contour setup was installed")
            .discovery
            .observe_with_options(&self.document, &discovery_options)?;
        Ok(())
    }
    fn listen(
        self: &Rc<Self>,
        target: &EventTarget,
        name: &'static str,
        capture: bool,
        kind: EventKind,
    ) -> Result<Listener, JsValue> {
        let weak = Rc::downgrade(self);
        let callback = Closure::wrap(Box::new(move |event: Event| {
            if let Some(inner) = weak.upgrade() {
                inner.event(&event, kind);
            }
        }) as Box<dyn FnMut(Event)>);
        target.add_event_listener_with_callback_and_bool(
            name,
            callback.as_ref().unchecked_ref(),
            capture,
        )?;
        Ok(Listener {
            target: target.clone(),
            name,
            capture,
            callback,
        })
    }
    fn event(&self, event: &Event, kind: EventKind) {
        if self.stopped.get() {
            return;
        }
        if matches!(kind, EventKind::PageHide) {
            if !(owner_instance(&self.view, event.as_ref(), "PageTransitionEvent")
                && event
                    .unchecked_ref::<web_sys::PageTransitionEvent>()
                    .persisted())
            {
                self.stop();
            }
            return;
        }
        if self.updating.get() {
            self.deferred_dirty.set(true);
            self.deferred_refresh.set(true);
            return;
        }
        let target = match kind {
            EventKind::Interaction | EventKind::AnimationStart | EventKind::AnimationEnd => {
                // These listeners belong to Document, so the dispatch target is
                // a Node; adopted elements can retain a foreign realm prototype.
                let node = event
                    .target()
                    .expect("invariant: document listener receives a dispatched event")
                    .unchecked_into::<Node>();
                (node.node_type() == Node::ELEMENT_NODE).then(|| node.unchecked_into::<Element>())
            }
            EventKind::Schedule => None,
            EventKind::PageHide => {
                unreachable!("invariant: pagehide is handled before target lookup")
            }
        };
        {
            let mut state = self.state.borrow_mut();
            match kind {
                EventKind::Interaction => {
                    if event.type_() == "scroll" {
                        reposition_foregrounds(&state.records, |error| {
                            web_sys::console::warn_2(
                                &"Could not reposition contour foreground".into(),
                                error,
                            );
                        });
                    }
                    let mut chain = Vec::new();
                    let mut element = target;
                    while let Some(node) = element {
                        element = node.parent_element();
                        chain.push(node);
                    }
                    self.invalidate(Trigger::Interaction(chain));
                }
                EventKind::AnimationStart | EventKind::AnimationEnd => {
                    if owner_instance(&self.view, event.as_ref(), "TransitionEvent")
                        && let Some(node) = target
                    {
                        let transition = event.unchecked_ref::<web_sys::TransitionEvent>();
                        let property = transition.property_name();
                        if owned_paint_property(&property) && state.index(&node).is_some() {
                            return;
                        }
                        if property == "--zxs-contour-extent" {
                            if matches!(kind, EventKind::AnimationStart) {
                                if let Some((_, count)) =
                                    state.active.iter_mut().find(|(target, _)| *target == node)
                                {
                                    *count += 1;
                                } else {
                                    state.active.push((node, 1));
                                }
                            } else if let Some(index) =
                                state.active.iter().position(|(target, _)| *target == node)
                            {
                                if state.active[index].1 > 1 {
                                    state.active[index].1 -= 1;
                                } else {
                                    state.active.remove(index);
                                }
                            }
                        }
                    }
                }
                EventKind::Schedule | EventKind::PageHide => {}
            }
        }
        if !matches!(kind, EventKind::Interaction) {
            self.invalidate(Trigger::Passive);
        }
        self.schedule();
    }
    fn schedule(&self) {
        if self.stopped.get() || self.frame.get().is_some() {
            return;
        }
        if let Some(setup) = self.setup.get() {
            match self
                .view
                .request_animation_frame(setup.animation_frame.as_ref().unchecked_ref())
            {
                Ok(frame) => self.frame.set(Some(frame)),
                Err(error) => {
                    web_sys::console::warn_2(&"Could not schedule contour refresh".into(), &error)
                }
            }
        }
    }
    fn invalidate(&self, trigger: Trigger<Element>) {
        self.invalidation.borrow_mut().add(trigger);
    }
    fn refresh_now(&self) -> Result<(), JsValue> {
        if self.updating.get() {
            self.deferred_refresh.set(true);
            return Ok(());
        }
        if let Some(frame) = self.frame.take() {
            self.view.cancel_animation_frame(frame)?;
        }
        self.update()
    }
    fn observe(&self) -> Result<(), JsValue> {
        if self.stopped.get() {
            return Ok(());
        }
        if let Some(setup) = self.setup.get() {
            let options = MutationObserverInit::new();
            options.set_subtree(true);
            options.set_attributes(true);
            options.set_character_data(true);
            setup
                .observer
                .observe_with_options(&self.document, &options)?;
        }
        Ok(())
    }
    fn update(&self) -> Result<(), JsValue> {
        if self.stopped.get() {
            return Ok(());
        }
        if self.updating.replace(true) {
            self.deferred_refresh.set(true);
            return Ok(());
        }
        let setup = self
            .setup
            .get()
            .expect("invariant: active contour controller was installed");
        // refresh_now may run before native delivery; structural changes retain
        // both membership and transition-policy invalidation.
        let records = mutations::records(setup.discovery.take_records());
        self.membership
            .borrow_mut()
            .reconcile(&self.document, &records);
        if !records.is_empty() {
            self.invalidate(Trigger::Structure);
        }
        setup.observer.disconnect();
        let work = self.invalidation.borrow_mut().take();
        let mut writes = OutputWrites::new(Some(&setup.discovery));
        let result = self.state.borrow_mut().update(self, work, &mut writes);
        let batch = writes.finish();
        self.membership
            .borrow_mut()
            .reconcile(&self.document, &batch.records);
        let author_changes = batch.author_changes;
        self.updating.set(false);
        if self.deferred_stop.replace(false) {
            self.stop();
            return result;
        }
        if self.deferred_dirty.replace(false) || author_changes {
            self.invalidate(Trigger::Author);
        }
        let observed = self.observe();
        if self.deferred_refresh.replace(false)
            || author_changes
            || !self.state.borrow().active.is_empty()
        {
            self.schedule();
        }
        result.and(observed)
    }
    fn stop(&self) {
        if self.stopped.get() {
            return;
        }
        if self.updating.get() {
            self.deferred_stop.set(true);
            return;
        }
        self.stopped.set(true);
        self.deferred_dirty.set(false);
        self.deferred_refresh.set(false);
        if let Some(frame) = self.frame.take()
            && let Err(error) = self.view.cancel_animation_frame(frame)
        {
            web_sys::console::warn_2(&"Could not cancel contour refresh".into(), &error);
        }
        if let Some(setup) = self.setup.get() {
            setup.observer.disconnect();
            setup.discovery.disconnect();
            setup.resize.disconnect();
            for listener in &setup.listeners {
                listener.detach();
            }
        }
        self.membership.borrow_mut().clear();
        self.invalidation.borrow_mut().take();
        let mut state = self.state.borrow_mut();
        state.active.clear();
        let mut writes = OutputWrites::new(None);
        for record in &mut state.records {
            if let Err(error) = restore(record, &self.draft, true, &mut writes) {
                web_sys::console::warn_2(&"Could not restore contour-owned styles".into(), &error);
            }
        }
        state.records.clear();
        state.indices.clear();
        for (_, url) in state.images.drain(..) {
            if let Err(error) = web_sys::Url::revoke_object_url(&url) {
                web_sys::console::warn_2(&"Could not release a contour image URL".into(), &error);
            }
        }
        state.used_images.clear();
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct State {
    records: Vec<Record>,
    // DOM object identity is provided by the browser without mutating nodes.
    indices: js_sys::Map,
    images: Vec<(String, String)>,
    used_images: BTreeSet<String>,
    active: Vec<(Element, u32)>,
}
struct Saved {
    value: String,
    priority: String,
    applied: Option<String>,
}
struct Record {
    node: HtmlElement,
    saved: BTreeMap<String, Saved>,
    pending: BTreeMap<String, String>,
    transition_dirty: bool,
    original_state: Option<String>,
    original_reason: Option<String>,
    decoration: Option<HtmlElement>,
    foreground: Option<SvgsvgElement>,
    restored_style: Option<String>,
    applied_style: Option<String>,
    applied_paint: Option<BTreeMap<String, String>>,
}
impl Record {
    fn new(node: &HtmlElement) -> Self {
        Self {
            node: node.clone(),
            saved: BTreeMap::new(),
            pending: BTreeMap::new(),
            transition_dirty: true,
            original_state: node.get_attribute("data-zxs-contour-state"),
            original_reason: node.get_attribute("data-zxs-contour-reason"),
            decoration: None,
            foreground: None,
            restored_style: None,
            applied_style: None,
            applied_paint: None,
        }
    }
    fn put(&mut self, name: &str, value: impl Into<String>) -> Result<(), JsValue> {
        if !self.saved.contains_key(name) {
            self.saved.insert(
                name.to_owned(),
                Saved {
                    value: self.node.style().get_property_value(name)?,
                    priority: self.node.style().get_property_priority(name),
                    applied: None,
                },
            );
        }
        self.pending.insert(name.to_owned(), value.into());
        Ok(())
    }
    fn mark(&self, status: &str, reason: Option<&str>) -> Result<(), JsValue> {
        self.node.set_attribute("data-zxs-contour-state", status)?;
        if let Some(reason) = reason {
            self.node.set_attribute("data-zxs-contour-reason", reason)?;
        } else {
            self.node.remove_attribute("data-zxs-contour-reason")?;
        }
        Ok(())
    }
    fn commit_pending(&mut self, draft: &CssStyleDeclaration) -> Result<(), JsValue> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let style = self.node.style();
        let restored = style.css_text();
        if self.restored_style.as_ref() == Some(&restored)
            && self.applied_paint.as_ref() == Some(&self.pending)
        {
            if let Some(applied) = &self.applied_style {
                style.set_css_text(applied);
            }
        } else {
            draft.set_css_text(&restored);
            for (name, value) in &self.pending {
                draft.set_property_with_priority(name, value, "important")?;
            }
            style.set_css_text(&draft.css_text());
        }
        self.restored_style = Some(restored);
        self.applied_style = Some(style.css_text());
        self.applied_paint = Some(self.pending.clone());
        for name in self.pending.keys() {
            if let Some(saved) = self.saved.get_mut(name) {
                saved.applied = Some(style.get_property_value(name)?);
            }
        }
        self.pending.clear();
        Ok(())
    }
}

fn restore(
    record: &mut Record,
    draft: &CssStyleDeclaration,
    final_restore: bool,
    writes: &mut OutputWrites<'_>,
) -> Result<(), JsValue> {
    let style = record.node.style();
    if !final_restore
        && record.restored_style.is_some()
        && record.applied_style.as_ref() == Some(&style.css_text())
    {
        style.set_css_text(record.restored_style.as_deref().unwrap_or_default());
        for (name, saved) in &mut record.saved {
            if !TRANSITIONS.contains(&name.as_str()) {
                saved.applied = None;
            }
        }
    } else {
        draft.set_css_text(&style.css_text());
        for (name, saved) in &mut record.saved {
            if !final_restore && TRANSITIONS.contains(&name.as_str()) {
                continue;
            }
            let current = style.get_property_value(name)?;
            let priority = style.get_property_priority(name);
            if saved.applied.as_ref() != Some(&current) || priority != "important" {
                saved.value = current;
                saved.priority = priority;
            }
            if saved.value.is_empty() {
                draft.remove_property(name)?;
            } else {
                draft.set_property_with_priority(name, &saved.value, &saved.priority)?;
            }
            saved.applied = None;
        }
        if style.css_text() != draft.css_text() {
            style.set_css_text(&draft.css_text());
        }
    }
    record.pending.clear();
    if let Some(child) = &record.decoration {
        child.style().set_property("display", "none")?;
    }
    if let Some(child) = &record.foreground {
        child
            .style()
            .set_property_with_priority("display", "none", "important")?;
    }
    if final_restore {
        if let Some(child) = record.decoration.take() {
            writes.remove(&child)?;
        }
        if let Some(child) = record.foreground.take() {
            writes.remove(&child)?;
        }
        for (name, value) in [
            ("data-zxs-contour-state", &record.original_state),
            ("data-zxs-contour-reason", &record.original_reason),
        ] {
            if let Some(value) = value {
                record.node.set_attribute(name, value)?;
            } else {
                record.node.remove_attribute(name)?;
            }
        }
    }
    Ok(())
}

impl State {
    fn index(&self, node: &Element) -> Option<usize> {
        self.indices
            .get(node.as_ref())
            .as_f64()
            .map(|index| index as usize)
    }
    fn reindex(&mut self) {
        self.indices.clear();
        for (index, record) in self.records.iter().enumerate() {
            self.indices
                .set(record.node.as_ref(), &JsValue::from_f64(index as f64));
        }
    }
    fn record(&mut self, node: &HtmlElement) -> &mut Record {
        let index = self.index(node).unwrap_or_else(|| {
            self.records.push(Record::new(node));
            let index = self.records.len() - 1;
            self.indices
                .set(node.as_ref(), &JsValue::from_f64(index as f64));
            index
        });
        &mut self.records[index]
    }
    /// Write back the author's own transition declarations before reading them.
    fn restore_transitions(
        record: &mut Record,
        draft: &CssStyleDeclaration,
    ) -> Result<(), JsValue> {
        let style = record.node.style();
        draft.set_css_text(&style.css_text());
        for name in TRANSITIONS {
            if let Some(saved) = record.saved.get_mut(name) {
                let current = style.get_property_value(name)?;
                let priority = style.get_property_priority(name);
                if saved.applied.as_ref() != Some(&current) || priority != "important" {
                    saved.value = current;
                    saved.priority = priority;
                }
                if saved.value.is_empty() {
                    draft.remove_property(name)?;
                } else {
                    draft.set_property_with_priority(name, &saved.value, &saved.priority)?;
                }
                saved.applied = None;
            }
        }
        if style.css_text() != draft.css_text() {
            style.set_css_text(&draft.css_text());
        }
        Ok(())
    }
    /// Apply transition overrides to the record of `node`, creating it when absent.
    fn apply_transitions(
        &mut self,
        node: &HtmlElement,
        overrides: [String; 5],
        draft: &CssStyleDeclaration,
    ) -> Result<(), JsValue> {
        let record = self.record(node);
        for (name, value) in TRANSITIONS.iter().zip(overrides) {
            record.put(name, value)?;
        }
        record.transition_dirty = false;
        let style = record.node.style();
        draft.set_css_text(&style.css_text());
        for (name, value) in &record.pending {
            draft.set_property_with_priority(name, value, "important")?;
        }
        style.set_css_text(&draft.css_text());
        for name in record.pending.keys() {
            if let Some(saved) = record.saved.get_mut(name) {
                saved.applied = Some(style.get_property_value(name)?);
            }
        }
        record.pending.clear();
        Ok(())
    }
    fn image_url(&mut self, source: String) -> Result<String, JsValue> {
        self.used_images.insert(source.clone());
        if let Some((_, url)) = self.images.iter().find(|(image, _)| *image == source) {
            return Ok(format!("url(\"{url}\")"));
        }
        let parts = js_sys::Array::new();
        parts.push(&JsValue::from_str(&source));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("image/svg+xml");
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob)?;
        self.images.push((source, url.clone()));
        Ok(format!("url(\"{url}\")"))
    }
    fn update(
        &mut self,
        inner: &Inner,
        work: FrameWork<Element>,
        writes: &mut OutputWrites<'_>,
    ) -> Result<(), JsValue> {
        match work.transitions {
            TransitionScope::All => {
                for record in &mut self.records {
                    record.transition_dirty = true;
                }
            }
            TransitionScope::Chains(chain) => {
                for node in chain {
                    if let Some(index) = self.index(&node) {
                        self.records[index].transition_dirty = true;
                    }
                }
            }
            TransitionScope::Unchanged => {}
        }
        self.used_images.clear();
        let dirty: Vec<usize> = (0..self.records.len())
            .filter(|&index| {
                self.records[index].transition_dirty && self.records[index].node.is_connected()
            })
            .collect();
        let mut transitions = Vec::new();
        let mut candidates = Vec::new();
        let mut foregrounds = Vec::new();
        // Preserve the original record order and finish each transition policy
        // before restoring the next. Inline-style selectors (including sibling
        // combinators and :has()) can depend on another record's override, so
        // ancestry alone cannot establish an independent batch.
        let steps = dirty
            .iter()
            .flat_map(|index| {
                frame::TRANSITION_RECORD.map(|step| (std::slice::from_ref(index), step))
            })
            .chain(frame::FRAME.map(|step| (&[][..], step)));
        for (record_indices, step) in steps {
            match step {
                Step::TransitionRestore => {
                    for &index in record_indices {
                        Self::restore_transitions(&mut self.records[index], &inner.draft)?;
                    }
                }
                Step::TransitionRead => {
                    for &index in record_indices {
                        let css = inner
                            .view
                            .get_computed_style(&self.records[index].node)?
                            .ok_or_else(|| {
                                JsValue::from_str("Computed element style is unavailable")
                            })?;
                        transitions.push(read_transitions(&css)?);
                    }
                }
                Step::TransitionWrite => {
                    for (&index, lists) in record_indices.iter().zip(transitions.drain(..)) {
                        match frame::transition_overrides(&lists, &inner.all_properties) {
                            Some(overrides) => {
                                let node = self.records[index].node.clone();
                                self.apply_transitions(&node, overrides, &inner.draft)?;
                            }
                            None => self.records[index].transition_dirty = false,
                        }
                    }
                }
                Step::PaintRestore => {
                    let mut keep = Vec::new();
                    for mut record in self.records.drain(..) {
                        let detached = !record.node.is_connected();
                        if detached && let Some(setup) = inner.setup.get() {
                            setup.resize.unobserve(&record.node);
                        }
                        restore(&mut record, &inner.draft, detached, writes)?;
                        if !detached {
                            keep.push(record);
                        }
                    }
                    self.records = keep;
                    self.reindex();
                }
                Step::Qualify => self.qualify(inner, &mut candidates)?,
                Step::Paint => {
                    let selected = js_sys::Set::default();
                    for candidate in candidates.drain(..) {
                        self.paint_candidate(inner, candidate, &selected, writes)?;
                    }
                    let mut keep = Vec::new();
                    for mut record in self.records.drain(..) {
                        if selected.has(record.node.as_ref()) {
                            record.commit_pending(&inner.draft)?;
                            keep.push(record);
                        } else {
                            restore(&mut record, &inner.draft, true, writes)?;
                            if let Some(setup) = inner.setup.get() {
                                setup.resize.unobserve(&record.node);
                            }
                        }
                    }
                    self.records = keep;
                    self.reindex();
                }
                Step::ForegroundRead => {
                    for record in &self.records {
                        if let Some(offset) = foreground_offset(record)? {
                            foregrounds.push(offset);
                        }
                    }
                }
                Step::ForegroundWrite => {
                    for offset in foregrounds.drain(..) {
                        offset.write()?;
                    }
                }
                Step::ForegroundVerify => {
                    for record in &self.records {
                        if let Some(offset) = foreground_offset(record)?
                            && offset.moves()
                        {
                            foregrounds.push(offset);
                        }
                    }
                }
                Step::ForegroundCorrect => {
                    for offset in foregrounds.drain(..) {
                        offset.write()?;
                    }
                }
            }
        }
        let inactive = self
            .images
            .iter()
            .filter(|(source, _)| !self.used_images.contains(source))
            .count();
        let mut remove = inactive.saturating_sub(96);
        self.images.retain(|(source, url)| {
            if remove > 0 && !self.used_images.contains(source) {
                remove -= 1;
                if let Err(error) = web_sys::Url::revoke_object_url(url) {
                    web_sys::console::warn_2(
                        &"Could not release a cached contour image URL".into(),
                        &error,
                    );
                }
                false
            } else {
                true
            }
        });
        self.active.retain(|(target, _)| {
            target.is_connected() && target.owner_document().as_ref() == Some(&inner.document)
        });
        Ok(())
    }
    /// Read every registered node's qualification, snapshot, size and
    /// fragments in document order. A first-seen candidate whose author
    /// transitions include an owned paint property gets its override written
    /// before its own snapshot is read, exactly as before: the override can
    /// cancel a running paint transition and so change what this node and
    /// later (inheriting) nodes report.
    fn qualify(&mut self, inner: &Inner, candidates: &mut Vec<Candidate>) -> Result<(), JsValue> {
        let mut members = inner.membership.borrow_mut();
        for member in members.members() {
            if member.node.has_attribute("data-zxs-contour-decoration") {
                continue;
            }
            let Some(live) = member.style(&inner.view)? else {
                continue;
            };
            let node = &member.node;
            if number(
                &node
                    .get_attribute("data-zxs-contour-extent")
                    .unwrap_or(live.get_property_value("--zxs-contour-extent")?),
            ) <= 0.0
            {
                continue;
            }
            if self.index(node).is_none()
                && let Some(overrides) =
                    frame::transition_overrides(&read_transitions(&live)?, &inner.all_properties)
            {
                self.apply_transitions(node, overrides, &inner.draft)?;
            }
            let css = paint::snapshot(&live)?;
            let width = measured_dimension(node, &css, "width");
            let height = measured_dimension(node, &css, "height");
            let fragments = node.get_client_rects().length();
            candidates.push(Candidate {
                node: node.clone(),
                css,
                width,
                height,
                fragmented: fragments > 1,
                visible: fragments > 0,
            });
        }
        Ok(())
    }
    fn paint_candidate(
        &mut self,
        inner: &Inner,
        candidate: Candidate,
        selected: &js_sys::Set,
        writes: &mut OutputWrites<'_>,
    ) -> Result<(), JsValue> {
        let Candidate {
            node,
            css,
            width,
            height,
            fragmented,
            visible,
        } = candidate;
        let raw = node
            .get_attribute("data-zxs-contour-extent")
            .unwrap_or_else(|| css.get("--zxs-contour-extent").trim().to_owned());
        let mode = node
            .get_attribute("data-zxs-contour")
            .unwrap_or_else(|| css.get("--zxs-contour-mode").trim().to_owned());
        if mode == "none" || number(&raw) <= 0.0 {
            return Ok(());
        }
        selected.add(node.as_ref());
        if let Some(setup) = inner.setup.get() {
            setup.resize.observe(&node);
        }
        macro_rules! fallback {
            ($reason:expr) => {{
                self.record(&node).mark("fallback", Some($reason))?;
                return Ok(());
            }};
        }
        if number(css.get("--zxs-contour-foreground")) > 0.0 && !visible {
            fallback!("not-visible");
        }
        if width <= 0.0 || height <= 0.0 {
            fallback!("not-visible");
        }
        if fragmented {
            fallback!("fragmented-inline");
        }
        if ["table", "table-row", "table-cell"].contains(&css.get("display")) {
            fallback!("table-layout");
        }
        if css.get("mask-image") != "none" {
            fallback!("existing-mask");
        }
        if css.get("border-image-source") != "none" {
            fallback!("existing-border-image");
        }
        let extent = if raw.ends_with('%') {
            number(&raw) * width.min(height) / 100.0
        } else {
            number(&raw)
        };
        let Ok(geometry) = geometry::create(width, height, extent, 0.02) else {
            fallback!("invalid-geometry");
        };
        let Some(layers) = paint::shadows(css.get("box-shadow")) else {
            fallback!("inset-or-unsupported-shadow");
        };
        let has_image = css.get("background-image") != "none";
        let has_backdrop = css.get("backdrop-filter") != "none";
        let background_child = has_image || has_backdrop;
        let tag = node.tag_name();
        let media = ["IMG", "VIDEO", "CANVAS"].contains(&tag.as_str());
        let replaced =
            ["INPUT", "TEXTAREA", "SELECT", "IMG", "VIDEO", "CANVAS"].contains(&tag.as_str());
        if background_child && replaced && !media {
            fallback!("native-background-image-or-backdrop");
        }
        let clipping = mode == "clip"
            || ["hidden", "clip"].contains(&css.get("overflow-x"))
            || ["hidden", "clip"].contains(&css.get("overflow-y"));
        let border_width = number(css.get("border-top-width"));
        let foreground = number(css.get("--zxs-contour-foreground")) > 0.0
            && clipping
            && border_width > 0.0
            && !replaced
            && SIDES.iter().all(|side| {
                css.get(&format!("border-{side}-style")) == "solid"
                    && number(css.get(&format!("border-{side}-width"))) == border_width
                    && css.get(&format!("border-{side}-color")) == css.get("border-top-color")
            });
        let foreground_path = if foreground {
            match geometry::offset_path(&geometry, -border_width) {
                Ok(path) => Some(geometry.path.clone() + &path),
                Err(error) => {
                    web_sys::console::warn_2(
                        &"Contour foreground geometry is unsupported".into(),
                        &error.into(),
                    );
                    fallback!("invalid-foreground-inset");
                }
            }
        } else {
            None
        };
        let painted = match paint::decoration(
            &geometry,
            &css,
            &layers,
            background_child && !media,
            foreground,
        ) {
            Ok(Some(paint)) => paint,
            Ok(None) => {
                fallback!("unsupported-border-or-shadow-inset");
            }
            Err(error) => {
                web_sys::console::warn_2(
                    &"Contour paint geometry is unsupported".into(),
                    &error.into(),
                );
                fallback!("invalid-paint-geometry");
            }
        };
        let border_image_slice = painted.border_image_slice();
        let painted_url = self.image_url(painted.image)?;
        let border_widths: Vec<_> = SIDES
            .iter()
            .map(|side| css.get(&format!("border-{side}-width")).to_owned())
            .collect();
        let visible_outline = number(css.get("outline-width")) > 0.0
            && !["none", "hidden"].contains(&css.get("outline-style"));
        let exact_control = ["INPUT", "TEXTAREA", "SELECT", "BUTTON"].contains(&tag.as_str())
            && !background_child
            && !foreground
            && layers.is_empty()
            && !visible_outline
            && border_widths.iter().any(|width| number(width) > 0.0);
        let mask_paint = if clipping {
            let margin = painted.margin.max(
                (number(css.get("outline-width")) + number(css.get("outline-offset")).abs() + 4.0)
                    .ceil(),
            );
            Some((margin, self.image_url(paint::mask(&geometry, margin))?))
        } else {
            None
        };
        let record = self.record(&node);
        if let Some(path) = foreground_path {
            if record.foreground.is_none() {
                let child = inner
                    .document
                    .create_element_ns(Some("http://www.w3.org/2000/svg"), "svg")?
                    .unchecked_into::<SvgsvgElement>();
                child.set_attribute("data-zxs-contour-decoration", "foreground")?;
                child.set_attribute("aria-hidden", "true")?;
                child.set_attribute("focusable", "false")?;
                let path = inner
                    .document
                    .create_element_ns(Some("http://www.w3.org/2000/svg"), "path")?;
                child.append_child(&path)?;
                record.foreground = Some(child);
            }
            let child = record.foreground.as_ref().unwrap();
            if child.parent_element().as_ref() != Some(node.as_ref())
                || node.last_element_child().as_ref() != Some(child.as_ref())
            {
                writes.append(&node, child)?;
            }
            child.set_attribute("viewBox", &format!("0 0 {} {}", fmt(width), fmt(height)))?;
            child.style().set_css_text(&format!("all:initial!important;display:block!important;position:fixed!important;left:0!important;top:0!important;width:{}px!important;height:{}px!important;max-width:none!important;max-height:none!important;pointer-events:none!important;visibility:inherit!important;z-index:2147483647!important;overflow:visible!important;", fmt(width), fmt(height)));
            let path_node = child
                .first_element_child()
                .ok_or_else(|| JsValue::from_str("Foreground path is missing"))?;
            path_node.set_attribute("d", &path)?;
            // Attribute style text is blocked by the application's style-src
            // policy. Use CSSOM, as for the SVG host, so the border keeps its
            // even-odd hole instead of becoming the default solid black fill.
            let path_style = path_node.unchecked_ref::<web_sys::SvgElement>().style();
            path_style.set_property_with_priority(
                "fill",
                css.get("border-top-color"),
                "important",
            )?;
            path_style.set_property_with_priority("fill-rule", "evenodd", "important")?;
            path_style.set_property_with_priority("stroke", "none", "important")?;
        }
        if background_child && !media {
            if record.decoration.is_none() {
                let child = inner
                    .document
                    .create_element("div")?
                    .unchecked_into::<HtmlElement>();
                child.set_attribute("data-zxs-contour-decoration", "")?;
                child.set_attribute("aria-hidden", "true")?;
                record.decoration = Some(child);
            }
            let child = record.decoration.as_ref().unwrap();
            if child.parent_element().as_ref() != Some(node.as_ref()) {
                writes.insert_before(&node, child, node.first_child().as_ref())?;
            }
            let style = child.style();
            style.set_css_text("position:absolute;pointer-events:none;z-index:-1;margin:0;padding:0;border:0;display:block;");
            style.set_property(
                "inset",
                &SIDES
                    .iter()
                    .map(|side| format!("{}px", -number(css.get(&format!("border-{side}-width")))))
                    .collect::<Vec<_>>()
                    .join(" "),
            )?;
            style.set_property("box-sizing", "border-box")?;
            for side in SIDES {
                style.set_property(
                    &format!("border-{side}"),
                    &format!(
                        "{} solid transparent",
                        css.get(&format!("border-{side}-width"))
                    ),
                )?;
                style.set_property(
                    &format!("padding-{side}"),
                    css.get(&format!("padding-{side}")),
                )?;
            }
            for name in [
                "background-color",
                "background-image",
                "background-position",
                "background-size",
                "background-repeat",
                "background-attachment",
                "background-origin",
                "background-clip",
                "background-blend-mode",
            ] {
                style.set_property(name, css.get(name))?;
            }
            style.set_property(
                "background-image",
                &format!("{painted_url}, {}", css.get("background-image")),
            )?;
            style.set_property(
                "background-position",
                &format!(
                    "{}px {}px, {}",
                    -painted.margin,
                    -painted.margin,
                    css.get("background-position")
                ),
            )?;
            style.set_property(
                "background-size",
                &format!(
                    "{}px {}px, {}",
                    fmt(width + painted.margin * 2.0),
                    fmt(height + painted.margin * 2.0),
                    css.get("background-size")
                ),
            )?;
            for (name, prefix) in [
                ("background-repeat", "no-repeat"),
                ("background-origin", "border-box"),
                ("background-clip", "border-box"),
                ("background-blend-mode", "normal"),
            ] {
                style.set_property(name, &format!("{prefix}, {}", css.get(name)))?;
            }
            style.set_property("backdrop-filter", css.get("backdrop-filter"))?;
            style.set_property("clip-path", &format!("path(\"{}\")", geometry.path))?;
            if css.get("position") == "static" {
                record.put("position", "relative")?;
            }
            record.put("isolation", "isolate")?;
        }
        record.put("background-color", "transparent")?;
        if !media {
            record.put("background-image", "none")?;
        }
        if has_backdrop {
            record.put("backdrop-filter", "none")?;
        }
        record.put("box-shadow", "none")?;
        record.put("border-radius", "0px")?;
        for (index, side) in SIDES.iter().enumerate() {
            record.put(&format!("border-{side}-width"), &border_widths[index])?;
            record.put(&format!("border-{side}-style"), "solid")?;
            record.put(&format!("border-{side}-color"), "transparent")?;
        }
        if exact_control {
            record.put("background-image", &painted_url)?;
            record.put(
                "background-size",
                format!(
                    "{}px {}px",
                    fmt(width + painted.margin * 2.0),
                    fmt(height + painted.margin * 2.0)
                ),
            )?;
            record.put(
                "background-position",
                format!("{}px {}px", -painted.margin, -painted.margin),
            )?;
            for (name, value) in [
                ("background-repeat", "no-repeat"),
                ("background-attachment", "scroll"),
                ("background-origin", "border-box"),
                ("background-clip", "border-box"),
                ("background-blend-mode", "normal"),
            ] {
                record.put(name, value)?;
            }
        }
        record.put(
            "border-image-source",
            if exact_control { "none" } else { &painted_url },
        )?;
        record.put("border-image-slice", border_image_slice)?;
        record.put(
            "border-image-width",
            format!("{}px", painted.slice.unwrap_or(painted.margin)),
        )?;
        record.put(
            "border-image-outset",
            if exact_control {
                "0".to_owned()
            } else {
                format!("{}px", painted.margin)
            },
        )?;
        record.put("border-image-repeat", "stretch")?;
        if let Some((margin, url)) = mask_paint {
            record.put("mask-image", url)?;
            record.put(
                "mask-clip",
                if exact_control {
                    "border-box"
                } else {
                    "no-clip"
                },
            )?;
            record.put("mask-origin", "border-box")?;
            record.put(
                "mask-size",
                format!(
                    "{}px {}px",
                    fmt(width + margin * 2.0),
                    fmt(height + margin * 2.0)
                ),
            )?;
            record.put("mask-position", format!("{}px {}px", -margin, -margin))?;
            record.put("mask-repeat", "no-repeat")?;
        }
        record.mark(if clipping { "clip" } else { "paint" }, None)
    }
}

struct Candidate {
    node: HtmlElement,
    css: CssSnapshot,
    width: f64,
    height: f64,
    fragmented: bool,
    visible: bool,
}
fn read_transitions(css: &CssStyleDeclaration) -> Result<TransitionLists, JsValue> {
    Ok(TransitionLists::parse(
        &css.get_property_value(TRANSITIONS[0])?,
        [
            css.get_property_value(TRANSITIONS[1])?,
            css.get_property_value(TRANSITIONS[2])?,
            css.get_property_value(TRANSITIONS[3])?,
            css.get_property_value(TRANSITIONS[4])?,
        ]
        .each_ref()
        .map(String::as_str),
    ))
}
fn measured_dimension(node: &HtmlElement, css: &CssSnapshot, axis: &str) -> f64 {
    let value = css.get(axis);
    let definite = value.strip_suffix("px").filter(|number| {
        !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit() || c == '.')
            && number.parse::<f64>().is_ok()
    });
    if let Some(value) = definite {
        let mut size = number(value);
        if css.get("box-sizing") != "border-box" {
            let sides = if axis == "width" {
                ["left", "right"]
            } else {
                ["top", "bottom"]
            };
            for side in sides {
                size += number(css.get(&format!("padding-{side}")))
                    + number(css.get(&format!("border-{side}-width")));
            }
        }
        if size != 0.0 {
            return size;
        }
    }
    let rect = node.get_bounding_client_rect();
    if axis == "width" {
        rect.width()
    } else {
        rect.height()
    }
}
/// A foreground offset computed from reads only. The SVG is fixed and out of
/// flow, so writing it normally moves nothing else; inside a transformed or
/// filtered scroll container it can still change scrollable overflow, so the
/// frame re-reads every foreground once and corrects those that moved.
struct ForegroundOffset {
    style: CssStyleDeclaration,
    current: (f64, f64),
    left: f64,
    top: f64,
}
impl ForegroundOffset {
    fn moves(&self) -> bool {
        fmt(self.left) != fmt(self.current.0) || fmt(self.top) != fmt(self.current.1)
    }
    fn write(self) -> Result<(), JsValue> {
        self.style.set_property_with_priority(
            "left",
            &format!("{}px", fmt(self.left)),
            "important",
        )?;
        self.style
            .set_property_with_priority("top", &format!("{}px", fmt(self.top)), "important")
    }
}
/// Reposition every foreground with all reads before the first write, then
/// re-read once and correct the foregrounds that the first writes moved.
fn reposition_foregrounds(records: &[Record], mut warn: impl FnMut(&JsValue)) {
    for verify in [false, true] {
        let mut offsets = Vec::new();
        for record in records {
            match foreground_offset(record) {
                Ok(Some(offset)) if !verify || offset.moves() => offsets.push(offset),
                Ok(_) => {}
                Err(error) => warn(&error),
            }
        }
        for offset in offsets {
            if let Err(error) = offset.write() {
                warn(&error);
            }
        }
    }
}
fn foreground_offset(record: &Record) -> Result<Option<ForegroundOffset>, JsValue> {
    let Some(foreground) = record.foreground.as_ref().filter(|foreground| {
        foreground.is_connected()
            && foreground
                .style()
                .get_property_value("display")
                .unwrap_or_default()
                != "none"
    }) else {
        return Ok(None);
    };
    let Some(matrix) = foreground.get_screen_ctm() else {
        return Ok(None);
    };
    let (a, b, c, d, e, f) = (
        f64::from(matrix.a()),
        f64::from(matrix.b()),
        f64::from(matrix.c()),
        f64::from(matrix.d()),
        f64::from(matrix.e()),
        f64::from(matrix.f()),
    );
    let determinant = a * d - b * c;
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return Ok(None);
    }
    let Some(viewbox) = foreground.view_box().base_val() else {
        return Ok(None);
    };
    let (width, height) = (f64::from(viewbox.width()), f64::from(viewbox.height()));
    let rect = record.node.get_bounding_client_rect();
    let x = rect.left() - (a * width).min(0.0) - (c * height).min(0.0) - e;
    let y = rect.top() - (b * width).min(0.0) - (d * height).min(0.0) - f;
    let style = foreground.style();
    let current = (
        number(&style.get_property_value("left")?),
        number(&style.get_property_value("top")?),
    );
    Ok(Some(ForegroundOffset {
        style,
        current,
        left: current.0 + (d * x - c * y) / determinant,
        top: current.1 + (a * y - b * x) / determinant,
    }))
}
