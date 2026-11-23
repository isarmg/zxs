//! The one installed v20 shared account has one new or historical operation.
//! The command never selects an alternate site, identity, database or vault.
use crate::sqlite_install::pair::{
    command::{key, sha, value},
    rust_enroll::manifest::Intent,
};
use std::ffi::OsString;

#[derive(Debug)]
pub(crate) struct Selection {
    providers: Option<([u8; 32], [u8; 32])>,
}
impl Selection {
    pub(crate) fn parse(mut it: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        if !cfg!(feature = "private-fresh-install") {
            return Err("private v20 shared-account maintenance is disabled");
        }
        let intent = match it.next().as_deref().and_then(|s| s.to_str()) {
            Some("new") => Intent::Fresh,
            Some("receipt") => Intent::Lookup,
            _ => return Err("choose reset-account new or receipt explicitly"),
        };
        let op = key(&mut it, "--op")?;
        let approval = key(&mut it, "--approval")?;
        let previous = key(&mut it, "--previous")?;
        let generation = value(&mut it, "--previous-generation")?;
        generation
            .to_str()
            .filter(|n| {
                n.len() <= 19
                    && n.as_bytes()
                        .first()
                        .is_some_and(|v| matches!(v, b'1'..=b'9'))
                    && n.bytes().all(|v| v.is_ascii_digit())
            })
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| (1..i64::MAX).contains(n))
            .ok_or("expected the exact positive previous shared-account generation")?;
        sha(&mut it, "--helper-sha256")?;
        let providers = if intent == Intent::Fresh {
            Some((
                sha(&mut it, "--gcrypt-sha256")?,
                sha(&mut it, "--gpgerror-sha256")?,
            ))
        } else {
            None
        };
        if it.next().is_some() || op == approval || previous == op {
            return Err("expected just the exact ordered reset and independent digests");
        }
        Ok(Self { providers })
    }
}
fn hex(bytes: &[u8]) -> String {
    crate::fs_tools::lower_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_reset_only_fixed_operation_order_and_exact_expected_predecessor() {
        let parse = |a: &[&str]| Selection::parse(a.iter().map(|s| OsString::from(*s)));
        let (op, approval, previous, d) =
            (hex(&[1; 16]), hex(&[2; 16]), hex(&[3; 16]), hex(&[4; 32]));
        let base = [
            "receipt",
            "--op",
            &op,
            "--approval",
            &approval,
            "--previous",
            &previous,
            "--previous-generation",
            "2",
            "--helper-sha256",
            &d,
        ];
        if !cfg!(feature = "private-fresh-install") {
            assert!(parse(&base).is_err());
            return;
        }
        assert!(parse(&base).unwrap().providers.is_none());
        let mut branch = base.to_vec();
        branch[0] = "new";
        assert!(parse(&branch).is_err());
        branch.extend(["--gcrypt-sha256", &d, "--gpgerror-sha256", &d]);
        assert!(parse(&branch).unwrap().providers.is_some());
        branch[0] = "receipt";
        assert!(parse(&branch).is_err());
        for bad in ["0", "01", "+2", "9223372036854775807", "٢"] {
            let mut wrong = base;
            wrong[8] = bad;
            assert!(parse(&wrong).is_err());
        }
        for extra in [
            "--password",
            "--system",
            "--site",
            "--login",
            "--resume",
            "--delete",
            "--web",
        ] {
            let mut wrong = base.to_vec();
            wrong.extend([extra, "fixture"]);
            assert!(parse(&wrong).is_err());
        }
        let mut no_resume = base;
        no_resume[0] = "resume";
        assert!(parse(&no_resume).is_err());
    }
}
