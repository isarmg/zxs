//! Read the exact original INIT on the once-selected SYSTEM alone. No data
//! volume is opened, inferred from SQL, synchronized or granted a new role.
use super::{
    DATABASE, MARKER, bad,
    catalog::{self, Canonical, Catalog},
    engine::{self, Access},
    markers::SystemMark,
    originals::LockedSystem,
    request::{Id, Request},
    reservation::Existing,
};
use std::io;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Receipt {
    pub(super) pool: Id,
    pub(super) committed_sha: Option<[u8; 32]>,
}

pub(super) fn read(system: &LockedSystem, original: &Request) -> io::Result<Receipt> {
    let template = Canonical::new()?;
    let db = Existing::initial_receipt(system, original)?;
    let source = system.source();
    let slots = || source.controls(&[DATABASE.as_bytes(), MARKER]);
    let initial = slots()?;
    if !initial[0] {
        return Err(bad("the original INIT database is not present"));
    }
    let marker = if initial[1] {
        Some(SystemMark::before_sql(source)?)
    } else {
        None
    };
    let verify = |claim: &Catalog| -> io::Result<()> {
        system.require()?;
        db.sidecars_absent()?;
        if slots()? != initial {
            return Err(bad(
                "the original SYSTEM controls changed during this receipt inquiry",
            ));
        }
        match &marker {
            Some(m) => m.verify(
                source,
                claim
                    .markers
                    .first()
                    .ok_or_else(|| bad("missing original SYSTEM claim"))?,
            ),
            None if claim.committed => Err(bad(
                "committed INIT lacks its required original SYSTEM marker",
            )),
            None => Ok(()),
        }
    };
    let failure = |_| bad("this original INIT receipt could not be confirmed");
    let mut c = engine::existing(&db, Access::Read)?;
    let tx = c.transaction().map_err(failure)?;
    let claim = catalog::initial_receipt(&tx, &template, source, original)?;
    verify(&claim)?;
    engine::original_name(&tx)?;
    tx.commit().map_err(failure)?;
    engine::original_name(&c)?;
    verify(&claim)?;
    c.close()
        .map_err(|_| bad("could not close the original read-only INIT observation"))?;
    verify(&claim)?;
    Ok(Receipt {
        pool: claim.pool,
        committed_sha: claim.committed.then(|| original.sha()),
    })
}
