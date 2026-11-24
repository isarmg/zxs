//! Native Linux dialog only returns a candidate path. It never opens a ZXS file.
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use eframe::egui::Context;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Workspace,
    CheckoutFolder,
    WorkingFile,
    FirstFile,
}

pub struct Selection {
    pub epoch: u64,
    pub field: Field,
    reply: Receiver<Option<PathBuf>>,
}

impl Selection {
    pub fn ask(ctx: &Context, epoch: u64, field: Field) -> io::Result<Self> {
        let (answer, reply) = mpsc::sync_channel(1);
        let repaint = ctx.clone();
        thread::Builder::new()
            .name("zxs-file-chooser".to_owned())
            .spawn(move || {
                // Use the current desktop's portal (or the crate's local
                // chooser fallback), with no cross-thread raw window pointer.
                let dialog = rfd::FileDialog::new().set_title(match field {
                    Field::Workspace => "Select the existing ZXS private directory",
                    Field::CheckoutFolder => "Select a folder for the new ordinary file",
                    Field::WorkingFile => "Select the ordinary working file",
                    Field::FirstFile => "Select the ordinary file to submit",
                });
                let path = match field {
                    Field::Workspace | Field::CheckoutFolder => dialog.pick_folder(),
                    Field::WorkingFile | Field::FirstFile => dialog.pick_file(),
                };
                let _ = answer.send(path);
                repaint.request_repaint();
            })?;
        Ok(Self {
            epoch,
            field,
            reply,
        })
    }

    pub fn poll(&self) -> Option<Option<PathBuf>> {
        match self.reply.try_recv() {
            Ok(path) => Some(path),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}
