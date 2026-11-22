//! Fixed synchronous boundary to the console's browser APIs. Import pointers
//! address only the given Rust slices during the call; the host retains none.
use super::route::MAX_FRAGMENT;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fault;

pub(crate) type Result<T> = std::result::Result<T, Fault>;

#[link(wasm_import_module = "zxs_console")]
unsafe extern "C" {
    fn find(id: *const u8, length: usize) -> u32;
    fn query(parent: u32, selector: *const u8, length: usize) -> u32;
    fn first(parent: u32) -> u32;
    fn share_count(roots: u32) -> i32;
    fn share_at(roots: u32, index: u32) -> u32;
    fn make(tag: *const u8, length: usize, svg: u32) -> u32;
    fn release(element: u32);
    fn place(parent: u32, child: u32, prepend: u32) -> u32;
    fn write_text(element: u32, bytes: *const u8, length: usize) -> u32;
    fn write_attr(element: u32, name: *const u8, name_len: usize, value: *const u8, value_len: usize, remove: u32) -> u32;
    fn move_focus(element: u32) -> u32;
    fn read_attr(element: u32, name: *const u8, name_len: usize, out: *mut u8, capacity: usize) -> i32;
    fn read_text(element: u32, out: *mut u8, capacity: usize) -> i32;
    fn read_value(element: u32, out: *mut u8, capacity: usize) -> i32;
    fn read_location(out: *mut u8, capacity: usize) -> i32;
    fn to_sign_in() -> u32;
    fn set_location(value: *const u8, length: usize) -> u32;
    fn begin_read(ticket: u64, path: *const u8, length: usize);
    fn begin_post(ticket: u64, kind: u32, form: *const u8, length: usize);
    fn cancel_read();
    fn row_count(group: u32) -> i32;
    fn read_row(group: u32, index: u32, field: u32, out: *mut u8, capacity: usize) -> i32;
}

fn status(value: u32) -> Result<()> {
    if value == 1 { Ok(()) } else { Err(Fault) }
}

fn read<const LIMIT: usize>(fill: impl FnOnce(&mut [u8; LIMIT]) -> i32) -> Result<Option<String>> {
    let mut bytes = [0; LIMIT];
    let size = fill(&mut bytes);
    if size == -1 {
        return Ok(None);
    }
    let actual = bytes.get(..usize::try_from(size).map_err(|_| Fault)?).ok_or(Fault)?;
    Ok(Some(std::str::from_utf8(actual).map_err(|_| Fault)?.to_owned()))
}

struct Held(u32);

impl Drop for Held {
    fn drop(&mut self) {
        // Each nonzero handle belongs to just one Held; its Rc shares it in Rust.
        unsafe { release(self.0) }
    }
}

#[derive(Clone)]
pub(crate) struct Element(Rc<Held>);

impl Element {
    pub(super) fn take(key: u32) -> Result<Self> {
        if key == 0 { Err(Fault) } else { Ok(Self(Rc::new(Held(key)))) }
    }

    fn key(&self) -> u32 { self.0.0 }

    pub(crate) fn by_id(id: &str) -> Result<Self> {
        Self::take(unsafe { find(id.as_ptr(), id.len()) })
    }

    pub(crate) fn query(&self, selector: &str) -> Result<Self> {
        Self::take(unsafe { query(self.key(), selector.as_ptr(), selector.len()) })
    }

    pub(crate) fn first(&self) -> Result<Self> {
        Self::take(unsafe { first(self.key()) })
    }

    pub(crate) fn shares(&self) -> Result<u32> {
        u32::try_from(unsafe { share_count(self.key()) }).map_err(|_| Fault)
    }

    pub(crate) fn share_at(&self, index: u32) -> Result<Self> {
        Self::take(unsafe { share_at(self.key(), index) })
    }

    pub(crate) fn new(tag: &str, svg: bool) -> Result<Self> {
        Self::take(unsafe { make(tag.as_ptr(), tag.len(), u32::from(svg)) })
    }

    pub(crate) fn append_child(&self, child: &Self) -> Result<()> {
        status(unsafe { place(self.key(), child.key(), 0) })
    }

    pub(crate) fn prepend(&self, child: &Self) -> Result<()> {
        status(unsafe { place(self.key(), child.key(), 1) })
    }

    pub(crate) fn text(&self) -> Result<Option<String>> {
        read::<255>(|out| unsafe { read_text(self.key(), out.as_mut_ptr(), out.len()) })
    }

    pub(crate) fn attr(&self, name: &str) -> Result<Option<String>> {
        self.attr_with::<255>(name)
    }

    pub(crate) fn attr_with<const N: usize>(&self, name: &str) -> Result<Option<String>> {
        read::<N>(|out| unsafe {
            read_attr(self.key(), name.as_ptr(), name.len(), out.as_mut_ptr(), out.len())
        })
    }

    pub(crate) fn value<const N: usize>(&self) -> Result<String> {
        read::<N>(|out| unsafe { read_value(self.key(), out.as_mut_ptr(), out.len()) })?.ok_or(Fault)
    }

    pub(crate) fn set_text(&self, value: &str) -> Result<()> {
        status(unsafe { write_text(self.key(), value.as_ptr(), value.len()) })
    }

    pub(crate) fn set_attribute(&self, name: &str, value: &str) -> Result<()> {
        self.attribute(name, Some(value))
    }

    pub(crate) fn remove_attribute(&self, name: &str) -> Result<()> {
        self.attribute(name, None)
    }

    fn attribute(&self, name: &str, value: Option<&str>) -> Result<()> {
        let bytes = value.unwrap_or("");
        status(unsafe {
            write_attr(self.key(), name.as_ptr(), name.len(), bytes.as_ptr(), bytes.len(), u32::from(value.is_none()))
        })
    }

    pub(crate) fn focus(&self) -> Result<()> {
        status(unsafe { move_focus(self.key()) })
    }

    pub(crate) fn set_class_name(&self, class: &str) -> Result<()> {
        self.set_attribute("class", class)
    }
}

pub(crate) fn hash() -> Result<String> {
    read::<MAX_FRAGMENT>(|out| unsafe { read_location(out.as_mut_ptr(), out.len()) })?.ok_or(Fault)
}

pub(crate) fn sign_in() -> Result<()> { status(unsafe { to_sign_in() }) }

pub(crate) fn set_hash(value: &str) -> Result<()> {
    status(unsafe { set_location(value.as_ptr(), value.len()) })
}

pub(crate) fn request(ticket: u64, path: &str) {
    unsafe { begin_read(ticket, path.as_ptr(), path.len()) }
}

pub(crate) enum Post { Filter, Labels, Edit }

pub(crate) fn post(ticket: u64, purpose: Post, form: &str) {
    unsafe { begin_post(ticket, purpose as u32, form.as_ptr(), form.len()) }
}

pub(crate) fn cancel_request() {
    unsafe { cancel_read() }
}

// Borrowed from this completion only: ancestors, entries, directory header,
// and label rows. Typed decoding belongs to the application.
pub(crate) fn rows(group: u32) -> Option<u32> {
    u32::try_from(unsafe { row_count(group) }).ok()
}

pub(crate) fn field<const LIMIT: usize>(group: u32, index: u32, field: u32) -> Result<Option<String>> {
    read::<LIMIT>(|out| unsafe { read_row(group, index, field, out.as_mut_ptr(), out.len()) })
}
