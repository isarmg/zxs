//! Exact R3 reply DTOs; no filesystem locations or diagnostic internals.
use super::super::dispatch::Reply;
use crate::{
    container_store as core,
    headless_http::{Outcome, Problem},
};
use std::fmt::{self, Write as _};

// A metadata reply is all-or-error, never a silently truncated listing.
const MAX_REPLY: usize = 1024 * 1024;

pub(super) fn project(reply: Reply) -> Result<Outcome, Problem> {
    let mut out = Json(String::new());
    match reply {
        Reply::Node(bytes) => {
            return Ok(Outcome::Node {
                length: bytes.node.body.length,
                sha256: bytes.node.body.sha256,
                body: Box::new(bytes.file),
            });
        }
        Reply::Receipt(None) => return Ok(Outcome::ReceiptAbsent),

        Reply::Catalog(page) => out.catalog(&page),
        Reply::Detail(page) => out.detail(&page),
        Reply::Tags(tags) => out.array(&tags, |o, tag| o.string(tag)),
        Reply::BranchHistory(page) => out.history(&page),
        Reply::Mutation(receipt)
        | Reply::Receipt(Some(core::OperationReceipt::Mutation(receipt))) => out.receipt(&receipt),
        Reply::Recycle {
            selected,
            source_cleanup_pending,
        } => out.recycle(selected, Some(source_cleanup_pending)),
        Reply::Receipt(Some(core::OperationReceipt::Recycle(receipt))) => {
            out.recycle(receipt, None)
        }
    }
    .map_err(|_| Problem::Unavailable)?;
    Ok(Outcome::Json(out.0.into_bytes()))
}

struct Json(String);

impl fmt::Write for Json {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if value.len() > MAX_REPLY - self.0.len() {
            return Err(fmt::Error);
        }
        self.0.push_str(value);
        Ok(())
    }
}

impl Json {
    fn string(&mut self, value: &str) -> fmt::Result {
        self.write_char('"')?;
        for ch in value.chars() {
            match ch {
                '"' => self.write_str("\\\"")?,
                '\\' => self.write_str("\\\\")?,
                ch if ch < '\u{20}' => write!(self, "\\u{:04x}", u32::from(ch))?,
                ch => self.write_char(ch)?,
            }
        }
        self.write_char('"')
    }

    fn hex(&mut self, value: &[u8]) -> fmt::Result {
        self.write_char('"')?;
        for byte in value {
            write!(self, "{byte:02x}")?;
        }
        self.write_char('"')
    }

    fn id_or_null(&mut self, value: Option<core::Id>) -> fmt::Result {
        if let Some(id) = value {
            self.hex(&id)
        } else {
            self.write_str("null")
        }
    }

    fn array<T>(
        &mut self,
        values: &[T],
        mut emit: impl FnMut(&mut Self, &T) -> fmt::Result,
    ) -> fmt::Result {
        self.write_char('[')?;
        for (offset, value) in values.iter().enumerate() {
            if offset != 0 {
                self.write_char(',')?;
            }
            emit(self, value)?;
        }
        self.write_char(']')
    }

    fn catalog(&mut self, page: &core::CatalogPage) -> fmt::Result {
        self.write_str("{\"items\":")?;
        self.array(&page.items, |out, row| {
            out.write_str("{\"container_id\":")?;
            out.hex(&row.id)?;
            write!(out, ",\"sequence\":{},\"name\":", row.sequence)?;
            out.string(&row.name)?;
            out.write_char('}')
        })?;
        self.write_str(",\"next_after\":")?;
        self.id_or_null(page.next_after)?;
        self.write_char('}')
    }

    fn detail(&mut self, page: &core::DetailPage) -> fmt::Result {
        self.write_str("{\"container_id\":")?;
        self.hex(&page.id)?;
        write!(self, ",\"sequence\":{},\"name\":", page.sequence)?;
        self.string(&page.name)?;
        self.write_str(",\"tags\":")?;
        self.array(&page.tags, |out, tag| out.string(tag))?;
        self.write_str(",\"heads\":")?;
        self.array(&page.heads, |out, (branch, head)| {
            out.write_str("{\"branch\":")?;
            out.string(branch)?;
            out.write_str(",\"head\":")?;
            out.hex(head)?;
            out.write_char('}')
        })?;
        self.write_str(",\"next_after\":")?;
        if let Some(after) = &page.next_after {
            self.string(after)?;
        } else {
            self.write_str("null")?;
        }
        self.write_char('}')
    }

    fn history(&mut self, page: &core::HistoryPage) -> fmt::Result {
        self.write_str("{\"head\":")?;
        self.hex(&page.head)?;
        self.write_str(",\"nodes\":")?;
        self.array(&page.nodes, |out, (id, node)| out.node(id, node))?;
        self.write_str(",\"next\":")?;
        self.id_or_null(page.next)?;
        self.write_char('}')
    }

    fn node(&mut self, id: &core::NodeId, node: &core::Node) -> fmt::Result {
        self.write_str("{\"node_id\":")?;
        self.hex(id)?;
        self.write_str(",\"operation_id\":")?;
        self.hex(&node.request)?;
        self.write_str(",\"parent_node\":")?;
        self.id_or_null(node.prior)?;
        self.write_str(",\"original_branch\":")?;
        self.string(&node.original_branch)?;
        write!(
            self,
            ",\"sequence\":{},\"length\":{},\"sha256\":",
            node.sequence, node.body.length
        )?;
        self.hex(&node.body.sha256)?;
        self.write_char('}')
    }

    fn mutation(&mut self, value: core::Mutation) -> fmt::Result {
        self.write_str("{\"container_id\":")?;
        self.hex(&value.container)?;
        self.write_str(",\"operation_id\":")?;
        self.hex(&value.request)
    }

    fn receipt(&mut self, row: &core::Receipt) -> fmt::Result {
        self.mutation(row.mutation)?;
        write!(self, ",\"sequence\":{}", row.sequence)?;
        match &row.change {
            core::Change::Save {
                target,
                node,
                body,
                initial_name,
            } => {
                self.write_str(",\"kind\":\"save_node\",\"node_id\":")?;
                self.hex(node)?;
                write!(self, ",\"length\":{},\"sha256\":", body.length)?;
                self.hex(&body.sha256)?;
                self.write_str(",\"target\":")?;
                self.target(target, initial_name.as_deref())?;
            }
            core::Change::Branch { name, from } => {
                self.write_str(",\"kind\":\"create_branch\",\"branch\":")?;
                self.string(name)?;
                self.write_str(",\"from_node\":")?;
                self.hex(from)?;
            }
            core::Change::Rename {
                expected_sequence,
                name,
            } => {
                write!(
                    self,
                    ",\"kind\":\"rename\",\"expected_sequence\":{expected_sequence},\"name\":"
                )?;
                self.string(name)?;
            }
            core::Change::Tags {
                expected_sequence,
                tags,
            } => {
                write!(
                    self,
                    ",\"kind\":\"replace_tags\",\"expected_sequence\":{expected_sequence},\"tags\":"
                )?;
                self.array(tags, |o, tag| o.string(tag))?;
            }
        }
        self.write_char('}')
    }

    fn target(&mut self, target: &core::Target, first: Option<&str>) -> fmt::Result {
        match (target, first) {
            (
                core::Target::Branch {
                    name,
                    expected_head: None,
                },
                Some(initial),
            ) if name == core::MAIN => {
                self.write_str("{\"kind\":\"first_main\",\"name\":")?;
                self.string(initial)?;
            }
            (
                core::Target::Branch {
                    name,
                    expected_head: Some(head),
                },
                None,
            ) => {
                self.write_str("{\"kind\":\"branch\",\"branch\":")?;
                self.string(name)?;
                self.write_str(",\"expected_head\":")?;
                self.hex(head)?;
            }
            (
                core::Target::HistoricalMain {
                    new_branch,
                    original,
                },
                None,
            ) => {
                self.write_str("{\"kind\":\"historical_main\",\"branch\":")?;
                self.string(new_branch)?;
                self.write_str(",\"from_node\":")?;
                self.hex(original)?;
            }
            _ => return Err(fmt::Error),
        }
        self.write_char('}')
    }

    fn recycle(&mut self, row: core::RecycleReceipt, cleanup: Option<bool>) -> fmt::Result {
        self.mutation(row.mutation)?;
        let pending = match cleanup {
            Some(true) => "true",
            Some(false) => "false",
            None => "null",
        };
        write!(
            self,
            ",\"kind\":\"recycle\",\"original_sequence\":{},\"source_cleanup_pending\":{pending}}}",
            row.original_sequence
        )
    }
}
