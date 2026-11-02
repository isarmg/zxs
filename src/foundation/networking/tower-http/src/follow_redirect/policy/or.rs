use super::{Action, Attempt, Policy};
use http::Request;

/// A redirection [`Policy`] that combines the results of two `Policy`s.
///
/// See [`PolicyExt::or`][super::PolicyExt::or] for more details.
#[derive(Clone, Copy, Debug, Default)]
pub struct Or<A, B> {
    a: A,
    b: B,
}

impl<A, B> Or<A, B> {
    pub(crate) fn new<Bd, E>(a: A, b: B) -> Self
    where
        A: Policy<Bd, E>,
        B: Policy<Bd, E>,
    {
        Or { a, b }
    }
}

impl<Bd, E, A, B> Policy<Bd, E> for Or<A, B>
where
    A: Policy<Bd, E>,
    B: Policy<Bd, E>,
{
    fn redirect(&mut self, attempt: &Attempt<'_>) -> Result<Action, E> {
        match self.a.redirect(attempt) {
            Ok(Action::Stop) | Err(_) => self.b.redirect(attempt),
            a => a,
        }
    }

    fn on_request(&mut self, request: &mut Request<Bd>) {
        self.a.on_request(request);
        self.b.on_request(request);
    }

    fn clone_body(&self, body: &Bd) -> Option<Bd> {
        self.a.clone_body(body).or_else(|| self.b.clone_body(body))
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/tower-http/unit/src/follow_redirect/policy/or/tests_45.rs"
));
