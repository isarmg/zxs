use super::*;
use crate::headless_auth::LoginError;

const SCOPE: Scope = Scope([1; 16], [2; 16]);

fn header(token: &str) -> String {
    format!("Bearer {token}")
}

#[test]
fn frozen_token_encoding_is_exactly_canonical_and_unpadded() {
    let token = encoded(&[0xff; 32]);
    assert_eq!(token.len(), 43);
    assert_eq!(token, "_".repeat(42) + "8");
    assert!(token_digest(&token).is_ok());
    assert_eq!(
        token_digest(&(token.clone() + "=")),
        Err(AuthError::Invalid)
    );
    assert_eq!(
        token_digest(&("_".repeat(42) + "9")),
        Err(AuthError::Invalid)
    );
    assert_eq!(
        token_digest(&("A".repeat(42) + "+")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn precise_pool_password_epoch_expiry_and_restart_never_reauthorize() {
    let sessions = Sessions::default();
    let now = Instant::now();
    let token = sessions.register(SCOPE, now, [7; 32]).unwrap();
    let expires = now + Duration::from_secs(EXPIRES_SECONDS);
    let auth = header(&token);
    assert_eq!(sessions.prove(SCOPE, now, &auth), Ok(expires));
    for different in [Scope([3; 16], [2; 16]), Scope([1; 16], [3; 16])] {
        assert_eq!(
            sessions.prove(different, now, &auth),
            Err(AuthError::Invalid)
        );
    }
    assert_eq!(
        Sessions::default().prove(SCOPE, now, &auth),
        Err(AuthError::Invalid)
    );
    assert_eq!(
        sessions.prove(
            SCOPE,
            expires.checked_sub(Duration::from_nanos(1)).unwrap(),
            &auth
        ),
        Ok(expires)
    );
    assert_eq!(
        sessions.prove(SCOPE, expires, &auth),
        Err(AuthError::Invalid)
    );
}

#[test]
fn table_refuses_duplicates_and_full_slots_without_displacing_valid_tokens() {
    let sessions = Sessions::default();
    let now = Instant::now();
    assert_eq!(
        sessions.register(SCOPE, now, [0; 32]),
        Err(AuthError::Unavailable)
    );
    let first = sessions.register(SCOPE, now, [1; 32]).unwrap();
    assert_eq!(
        sessions.register(SCOPE, now, [1; 32]),
        Err(AuthError::Unavailable)
    );
    for n in 2..=MAX_SESSIONS {
        sessions
            .register(SCOPE, now, [u8::try_from(n).unwrap(); 32])
            .unwrap();
    }
    assert_eq!(
        sessions.register(SCOPE, now, [255; 32]),
        Err(AuthError::Unavailable)
    );
    assert!(sessions.prove(SCOPE, now, &header(&first)).is_ok());
    let after = now + Duration::from_secs(EXPIRES_SECONDS);
    assert!(sessions.register(SCOPE, after, [255; 32]).is_ok());
    assert_eq!(
        sessions.prove(SCOPE, after, &header(&first)),
        Err(AuthError::Invalid)
    );
}

#[test]
fn separate_login_window_counts_attempts_and_returns_bounded_retry_after() {
    let sessions = Sessions::default();
    let now = Instant::now();
    for _ in 0..LOGIN_ATTEMPTS {
        assert_eq!(sessions.allow_login(|| now), Ok(()));
    }
    assert_eq!(sessions.allow_login(|| now), Err(LoginError::Throttled(60)));
    let before_end = (now + LOGIN_WINDOW)
        .checked_sub(Duration::from_millis(1))
        .unwrap();
    assert_eq!(
        sessions.allow_login(|| before_end),
        Err(LoginError::Throttled(1))
    );
    let token = sessions.register(SCOPE, now, [1; 32]).unwrap();
    assert!(sessions.prove(SCOPE, now, &header(&token)).is_ok());
    assert_eq!(sessions.allow_login(|| now + LOGIN_WINDOW), Ok(()));
}
