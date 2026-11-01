//! NT ACL projection from authorized logical metadata, never the host export.
use super::{authorized_fstat, bad, guarded, nt_status, sdk};
use std::{io, ptr};

const SUPPORTED_SECURITY_INFO: u32 = sdk::SECINFO_OWNER | sdk::SECINFO_GROUP | sdk::SECINFO_DACL;

/// # Safety
/// The host owns all SDK arguments for this callback. A non-null output points
/// to writable pointer storage; mem_ctx owns the returned SDK descriptor.
pub(super) unsafe extern "C" fn get(
    handle: *mut sdk::vfs_handle_struct,
    fsp: *mut sdk::files_struct,
    security_info: u32,
    mem_ctx: *mut sdk::TALLOC_CTX,
    output: *mut *mut sdk::security_descriptor,
) -> sdk::NTSTATUS {
    let result = guarded(|| {
        if output.is_null() {
            return Err(bad());
        }
        // SAFETY: The checked output is writable under the callback contract.
        unsafe { *output = ptr::null_mut() };
        if security_info & !SUPPORTED_SECURITY_INFO != 0 {
            return Err(bad());
        }
        let mut stat = sdk::SMB_STRUCT_STAT::default();
        // SAFETY: Reuse the real callback's token, open-handle and RPC authority
        // checks. No descriptor is built when that admission fails.
        unsafe { authorized_fstat(handle, fsp, &mut stat) }?;
        // SAFETY: Successful fstat established the live handle/connection. The
        // host keeps its share parameters immutable during this event callback.
        let parameters = unsafe { (*(*handle).conn).params };
        if parameters.is_null() {
            return Err(bad());
        }
        let mut owner = sdk::dom_sid::default();
        let mut group = sdk::dom_sid::default();
        // SAFETY: SDK SID conversion writes these exclusive POD outputs. It
        // uses Samba's account mapping, not an inferred or request-provided SID.
        unsafe {
            sdk::uid_to_sid(&mut owner, stat.st_ex_uid);
            sdk::gid_to_sid(&mut group, stat.st_ex_gid);
            *output = descriptor(
                mem_ctx,
                security_info,
                stat.st_ex_mode,
                [&owner, &group],
                (*parameters).service,
            )?;
        }
        Ok(0)
    });
    if result == 0 {
        return nt_status(0);
    }
    // SAFETY: The SDK maps only errors (even errno 0 maps to an error). guarded
    // isolates Rust panics and gives them EIO, never a successful descriptor.
    unsafe { sdk::map_nt_error_from_unix(*libc::__errno_location()) }
}

/// # Safety
/// mem_ctx is a live SDK allocation context (or null), and both SIDs are live
/// for this call. SDK constructors copy the local ACEs and SIDs before returning.
unsafe fn descriptor(
    mem_ctx: *mut sdk::TALLOC_CTX,
    security_info: u32,
    mode: sdk::mode_t,
    sids: [&sdk::dom_sid; 2],
    service: i32,
) -> io::Result<*mut sdk::security_descriptor> {
    let directory = match mode & libc::S_IFMT {
        libc::S_IFDIR => true,
        libc::S_IFREG => false,
        _ => return Err(bad()),
    };
    let mut acl = ptr::null_mut();
    if security_info & sdk::SECINFO_DACL != 0 {
        let mut aces: [sdk::security_ace; 3] = std::array::from_fn(|_| Default::default());
        // SAFETY: The SDK exports this immutable world SID. Zero group/other
        // mode bits remain zero masks; no fallback grants rights to everyone.
        let trustees = [sids[0], sids[1], unsafe { &sdk::global_sid_World }];
        let permissions = [mode & 0o700, (mode & 0o070) << 3, (mode & 0o007) << 6];
        for ((ace, trustee), permissions) in aces.iter_mut().zip(trustees).zip(permissions) {
            let mut kind = sdk::security_ace_type_SEC_ACE_TYPE_ACCESS_ALLOWED;
            // SAFETY: These SDK functions write the exclusive local outputs.
            // Use the host's canonical POSIX mapping: 0600 is not FILE_ALL and
            // must not acquire execute permission from a generic default ACL.
            unsafe {
                let mask = sdk::map_canon_ace_perms(service, &mut kind, permissions, directory);
                sdk::init_sec_ace(ace, trustee, kind, mask, 0);
            }
        }
        // SAFETY: Constructors copy the three live ACEs into SDK-owned memory.
        // Samba 4.22.11 security.idl defines NT4_ACL_REVISION as this
        // enum member; bindgen emits the enum, not its macro alias.
        acl = unsafe {
            sdk::make_sec_acl(
                mem_ctx,
                sdk::security_acl_revision_SECURITY_ACL_REVISION_NT4,
                aces.len(),
                aces.as_ptr(),
            )
        };
        if acl.is_null() {
            return Err(io::Error::from_raw_os_error(libc::ENOMEM));
        }
    }
    let mut size = 0;
    let owner = if security_info & sdk::SECINFO_OWNER != 0 {
        sids[0]
    } else {
        ptr::null()
    };
    let group = if security_info & sdk::SECINFO_GROUP != 0 {
        sids[1]
    } else {
        ptr::null()
    };
    // SAFETY: The SDK copies the requested SIDs and ACL, tying the result to
    // mem_ctx. The temporary ACL is then freed exactly once, also on failure.
    let result = unsafe { sdk::make_standard_sec_desc(mem_ctx, owner, group, acl, &mut size) };
    if !acl.is_null() {
        // SAFETY: This is the one temporary SDK allocation owned by this call.
        unsafe { sdk::_talloc_free(acl.cast(), c"zxs logical ACL temporary".as_ptr()) };
    }
    if result.is_null() {
        return Err(io::Error::from_raw_os_error(libc::ENOMEM));
    }
    // SAFETY: The new descriptor is exclusively owned until returned to Samba.
    unsafe { (*result).type_ |= sdk::SEC_DESC_DACL_PROTECTED as u16 };
    Ok(result)
}

#[cfg(test)]
#[path = "../../../../test/smb-vfs/unit/module/acl_tests.rs"]
mod tests;
