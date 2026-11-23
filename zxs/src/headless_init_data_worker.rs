//! A local fixed operation, separated from one-file import by entrypoint,
//! sealed order magic and authenticated message kind. No source FD is accepted.
use super::{Channel, Sha256, fixed, inherited_order, refused, step, variable};
use crate::headless_offline;
use std::{cell::Cell, io};

unsafe extern "C" {
    fn umask(mode: std::ffi::c_uint) -> std::ffi::c_uint;
}

pub(crate) fn run() -> io::Result<()> {
    // SAFETY: Linux x86_64 mode_t is an unsigned int. Only this single-purpose
    // exec'd child changes its mask; the caller and normal server are untouched.
    unsafe {
        umask(0o077);
    }
    let (socket, parent, raw) = inherited_order()?;
    let (fence, selected) = headless_offline::retain()?;
    let InitSelection {
        scope,
        pool,
        token,
        path,
        roles,
    } = selection(&raw, fence.binding)?;
    let mut hash = Sha256::new();
    hash.update(&raw)?;
    let channel = Channel {
        socket,
        parent,
        scope,
        digest: hash.finish(),
        kind: 46,
    };
    let nonce = Cell::new(0_u64);
    let result = selected.init_data(pool, token, &path, &roles, |state| {
        fence.require(state)?;
        let next = nonce.get().checked_add(1).ok_or_else(refused)?;
        nonce.set(next);
        channel.send(step(1, next))?;
        let (answer, fd) = channel.receive()?;
        if fd.is_some() || answer != step(2, next) {
            return Err(refused());
        }
        fence.require(state)
    });
    if let Err(error) = result {
        let mut reply = step(4, nonce.get());
        let why = error.to_string();
        for (index, byte) in why.bytes().take(55).enumerate() {
            reply[9] += 1;
            reply[10 + index] = if (b' '..=b'~').contains(&byte) {
                byte
            } else {
                b'?'
            };
        }
        channel.send(reply)?;
        return Err(error);
    }
    if nonce.get() == 0 {
        return Err(refused());
    }
    channel.send(step(3, nonce.get()))
}

struct InitSelection {
    scope: [u8; 32],
    pool: [u8; 16],
    token: [u8; 32],
    roles: Vec<[u64; 3]>,
    path: Vec<u8>,
}

fn selection(mut raw: &[u8], binding: [u8; 32]) -> io::Result<InitSelection> {
    if fixed::<8>(&mut raw)? != *b"ZXINITD1" || fixed::<32>(&mut raw)? != binding {
        return Err(refused());
    }
    let scope = fixed(&mut raw)?;
    let pool = fixed(&mut raw)?;
    let token = fixed(&mut raw)?;
    let [count] = fixed(&mut raw)?;
    if !(3..=64).contains(&count) {
        return Err(refused());
    }
    let mut roles = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let role = [
            u64::from_be_bytes(fixed(&mut raw)?),
            u64::from_be_bytes(fixed(&mut raw)?),
            u64::from_be_bytes(fixed(&mut raw)?),
        ];
        if role[1] == 0 || role[2] == 0 {
            return Err(refused());
        }
        roles.push(role);
    }
    let path = variable(&mut raw, super::PATH_BYTES)?;
    if !raw.is_empty()
        || scope == [0; 32]
        || pool == [0; 16]
        || token == [0; 32]
        || !path.starts_with(b"/")
        || path.len() < 2
        || path.contains(&0)
        || path[1..]
            .split(|&c| c == b'/')
            .any(|c| c.is_empty() || matches!(c, b"." | b".."))
    {
        return Err(refused());
    }
    Ok(InitSelection {
        scope,
        pool,
        token,
        roles,
        path,
    })
}

#[cfg(test)]
#[path = "../tests/unit/init_data_order.rs"]
mod tests;
