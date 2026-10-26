use super::*;

/// Real bindgen SDK objects with synthetic session data, never ABI stand-ins.
/// Boxes keep all pointers stable if the owning fixture moves.
pub(crate) struct AuthenticatedSession {
    pub(crate) connection: Box<sdk::connection_struct>,
    pub(crate) session: Box<sdk::auth_session_info>,
    pub(crate) token: Box<sdk::security_token>,
    pub(crate) info: Box<sdk::auth_user_info>,
    sids: Box<[sdk::dom_sid]>,
}
impl AuthenticatedSession {
    pub(crate) fn new() -> Self {
        // SAFETY: The fixed SDK exports these immutable SID constants; copies
        // are owned by this fixture for the lifetime of all nested pointers.
        let sids = unsafe { vec![sdk::global_sid_Authenticated_Users] };
        Self::with_sids(sids)
    }
    fn with_sids(sids: Vec<sdk::dom_sid>) -> Self {
        let mut sids = sids.into_boxed_slice();
        let mut token = Box::new(sdk::security_token {
            num_sids: sids.len() as _,
            sids: sids.as_mut_ptr(),
            ..Default::default()
        });
        let mut info = Box::new(sdk::auth_user_info {
            account_name: c"alice".as_ptr().cast_mut(),
            ..Default::default()
        });
        let mut session = Box::new(sdk::auth_session_info {
            security_token: &mut *token,
            info: &mut *info,
            ..Default::default()
        });
        let connection = Box::new(sdk::connection_struct {
            session_info: &mut *session,
            ..Default::default()
        });
        Self {
            connection,
            session,
            token,
            info,
            sids,
        }
    }
    pub(crate) fn handle(&mut self) -> sdk::vfs_handle_struct {
        sdk::vfs_handle_struct {
            conn: &mut *self.connection,
            ..Default::default()
        }
    }
    fn account(&mut self) -> std::io::Result<String> {
        let mut handle = self.handle();
        // SAFETY: All nested SDK objects and SID/string storage are owned and
        // live in this fixture; account_name returns only an owned string.
        unsafe { account_name(&mut handle) }
    }
}

#[test]
fn authenticated_token_not_callback_name_establishes_the_account() {
    let mut fixture = AuthenticatedSession::new();
    assert_eq!(fixture.account().unwrap(), "alice");
    fixture.info.account_name = c"actual-account".as_ptr().cast_mut();
    assert_eq!(fixture.account().unwrap(), "actual-account");
}

#[test]
fn guest_anonymous_and_missing_authenticated_membership_are_denied() {
    // SAFETY: Copy immutable official SDK SID constants into owned test data.
    let combinations = unsafe {
        [
            vec![sdk::global_sid_Anonymous],
            vec![sdk::global_sid_Builtin_Guests],
            vec![
                sdk::global_sid_Anonymous,
                sdk::global_sid_Authenticated_Users,
            ],
            vec![
                sdk::global_sid_Authenticated_Users,
                sdk::global_sid_Builtin_Guests,
            ],
            vec![],
        ]
    };
    for sids in combinations {
        let mut fixture = AuthenticatedSession::with_sids(sids);
        assert_eq!(
            fixture.account().unwrap_err().raw_os_error(),
            Some(libc::EACCES)
        );
    }
}

#[test]
fn incomplete_or_forced_host_identity_fails_closed() {
    for invalid in 0..7 {
        let mut fixture = AuthenticatedSession::new();
        match invalid {
            0 => fixture.connection.force_user = true,
            1 => fixture.connection.session_info = std::ptr::null_mut(),
            2 => fixture.session.security_token = std::ptr::null_mut(),
            3 => fixture.session.info = std::ptr::null_mut(),
            4 => fixture.token.sids = std::ptr::null_mut(),
            5 => fixture.token.num_sids = 0,
            6 => fixture.info.account_name = std::ptr::null_mut(),
            _ => unreachable!(),
        }
        let error = fixture.account().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(smb_protocol::client::error_errno(&error), libc::EACCES);
    }
}

#[test]
fn invalid_account_names_preserve_sources_and_map_to_access_denied() {
    for bytes in [b"\xff\0".as_slice(), b"\0", b" alice\0", b"alice=bob\0"] {
        let name = std::ffi::CStr::from_bytes_with_nul(bytes).unwrap();
        let mut fixture = AuthenticatedSession::new();
        fixture.info.account_name = name.as_ptr().cast_mut();
        let error = fixture.account().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(smb_protocol::client::error_errno(&error), libc::EACCES);
        let source = error
            .get_ref()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
        if bytes == b"\xff\0" {
            assert!(
                source
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<std::str::Utf8Error>()
                    .is_some()
            );
        }
    }
}

#[test]
fn replacing_a_live_session_with_guest_is_not_cached_as_authenticated() {
    let mut fixture = AuthenticatedSession::new();
    assert!(fixture.account().is_ok());
    // SAFETY: Copy an immutable SDK SID into the live fixture's owned array.
    fixture.sids[0] = unsafe { sdk::global_sid_Anonymous };
    assert_eq!(
        fixture.account().unwrap_err().raw_os_error(),
        Some(libc::EACCES)
    );
}
