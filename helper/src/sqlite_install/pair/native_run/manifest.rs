//! Root-owned sealed request adapter for the legacy worker flow.
pub(super) use crate::local_worker::sealed_owner;
use std::{fs::File,io};

pub(super) fn read_sealed(f:&File,max:usize)->io::Result<Vec<u8>>{sealed_owner(f,max,(0,0))}
