//! Authenticated form adapter for label filtering and edits. No FILE body I/O.
use super::{App, BAD, LOGIN_REQUIRED, REJECT, Reply, Request, Session, Token, UNAVAILABLE,
    access_refused, directory::{self, Query}, inactive_session, session_live_at, transport};
use crate::{label_text::{self, Label}, system::reader::account::file_labels::{Labels, Selected}};
use std::{fmt::Write as _, io, time::Instant};

pub(super) const FILTER: &str = "/ui/v1/df";
pub(super) const LABELS: &str = "/ui/v1/labels";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    List(Option<Label>),
    Set { name: Label, present: bool },
}

fn optional<T: std::str::FromStr>(text: &str) -> Option<Option<T>> {
    if text == "-" { Some(None) } else { text.parse().ok().map(Some) }
}

fn action(share: &str, parent: &str, file: &str, position: &str, scope: &str, verb: &str, label: &str)
    -> Option<(Selected, Action)>
{
    let selected = Selected {
        share: share.parse().ok()?, parent: optional(parent)?,
        file: file.parse().ok()?, position: position.parse().ok()?, scope: label_text::hex(scope)?,
    };
    let op = match verb {
        "list" if label == "-" => Action::List(None),
        "list" => Action::List(Some(Label::from_hex(label)?)),
        "add" | "remove" => Action::Set { name: Label::from_hex(label)?, present: verb == "add" },
        _ => return None,
    };
    Some((selected, op))
}

fn response(labels: Labels) -> io::Result<Reply> {
    let mut out = String::from("{\"labels\":[");
    for (i, label) in labels.names.iter().enumerate() {
        if i != 0 { out.push(','); }
        write!(out, "\"{}\"", label.encoded()).unwrap(); // Canonical lower hex.
    }
    out.push_str("],\"next\":");
    if let Some(next) = labels.next {
        write!(out, "\"{}\"", next.encoded()).unwrap();
    } else {
        out.push_str("null");
    }
    out.push('}');
    directory::json(out)
}

fn serve(s: &mut Session<'_>, selected: Selected, op: Action) -> io::Result<Reply> {
    match op {
        Action::List(after) => {
            let prepared = s.admin.file_labels(selected, after.as_ref()).and_then(response);
            s.finish_read(prepared)
        }
        Action::Set { name, present } => {
            let prepared = s.admin.set_file_label(selected, &name, present, || {
                if session_live_at(s.born, s.last, Instant::now()) {
                    Ok(())
                } else {
                    Err(inactive_session())
                }
            });
            // A late completion, successful or not, cannot revive this session.
            s.finish_read(prepared).map(|()| Reply::text("204 No Content", ""))
        }
    }
}

fn matches_csrf(s: &Session<'_>, csrf: &str) -> bool {
    Token::parse(csrf).is_some_and(|t| s.csrf.matches(&t))
}

impl App<'_> {
    pub(super) fn label_form(&mut self, mut q: Request) -> Reply {
        // App::handle has already checked the original session audience and expiry.
        if q.origin.as_deref() != Some(q.audience.origin.as_str()) { return REJECT(); }
        if q.content_type != "application/x-www-form-urlencoded" { return BAD(); }
        let Some(key) = q.cookie.as_ref() else { return LOGIN_REQUIRED(); };
        let Some(s) = self.sessions.get_mut(key) else { return LOGIN_REQUIRED(); };
        let (result, written) = if q.target == FILTER {
            let Some([csrf, share, parent, after, label]) =
                transport::fields(&mut q.body.0, ["csrf", "share", "parent", "after", "label"])
            else { return BAD(); };
            if !matches_csrf(s, csrf) { return REJECT(); }
            let Some(query) = Query::filtered(share, parent, after, label) else { return BAD(); };
            (directory::page(s, &query), false)
        } else {
            let Some([csrf, share, parent, file, position, scope, verb, name]) = transport::fields(
                &mut q.body.0, ["csrf", "share", "parent", "file", "position", "scope", "action", "label"],
            ) else { return BAD(); };
            if !matches_csrf(s, csrf) { return REJECT(); }
            let Some((selected, op)) = action(share, parent, file, position, scope, verb, name) else { return BAD(); };
            let written = matches!(op, Action::Set { .. });
            (serve(s, selected, op), written)
        };
        match result {
            Ok(reply) => reply,
            Err(e) if access_refused(&e) => { self.sessions.remove(key); LOGIN_REQUIRED() }
            Err(e) => match e.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::WouldBlock =>
                    Reply::text("409 Conflict", "文件或目录已变化，请重新打开。"),
                io::ErrorKind::PermissionDenied => REJECT(),
                _ if written => Reply::text("503 Service Unavailable", "标签修改结果未确认，请重新查看标签。"),
                _ => UNAVAILABLE(),
            },
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/management/file_labels.rs"]
mod tests;
