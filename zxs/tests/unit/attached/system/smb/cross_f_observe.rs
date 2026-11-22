//! The independent read-only witness for the fixed Linux movable FILE fixture.
use super::{Catalog, Connection, InitialReader, SmbSession, Tree, file_create, first_work::Current};
use super::super::{
    file_location::Place,
    volume::{OrdinaryRoot, RecycleRoot},
    work_record,
};
use crate::{
    FileId, NodeId, OperationId, PoolId, Role, VolumeId,
    fs::{directory_birth::DirectoryParent, invalid},
    hash::Sha256,
    identity::ShareId,
    ordinary::portable_birth::Birth,
};
use std::{io, path::Path};

const SYSTEM: &str = "/mnt/smb-move/system/fixture";
const SOURCE: &str = "/mnt/smb-move/source/fixture";
const TARGET: &str = "/mnt/smb-move/target/fixture";
const RECYCLE: &str = "/mnt/smb-move/recycle/fixture";
const FILE: &str = "zxs-normal-move.bin";
const BYTES: usize = 4096;

pub(crate) struct FixtureRequest<'a> {
    pub(crate) pool: PoolId,
    pub(crate) login: &'a str,
    pub(crate) from: ShareId,
    pub(crate) to: ShareId,
    pub(crate) file: FileId,
    pub(crate) nodes: [NodeId; 2],
    pub(crate) phase: Phase,
}

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Source,
    Target { move_op: OperationId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Retirement {
    NotMoved,
    Pending,
    Retired,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct FixtureProof {
    pub(crate) file: FileId,
    pub(crate) volume: VolumeId,
    pub(crate) position: OperationId,
    pub(crate) generation: i64,
    pub(crate) versions: [VersionProof; 2],
    pub(crate) retirement: Retirement,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VersionProof {
    pub(crate) node: NodeId,
    pub(crate) save: OperationId,
    pub(crate) revision: u64,
    pub(crate) bytes: u64,
    pub(crate) sha256: [u8; 32],
}

struct Cut {
    catalog: Catalog,
    birth: Birth,
    original: Place,
    selected: Place,
}

fn select(db: &Connection, catalog: Catalog, request: &FixtureRequest<'_>, from: &Tree<'_, '_>,
    to: &Tree<'_, '_>) -> io::Result<Cut> {
    from.require(db, &catalog, true)?;
    to.require(db, &catalog, true)?;
    let rows = catalog.rows();
    let known = |role, path, volume: Option<VolumeId>| rows.iter().any(|row|
        row.role == role && row.path == Path::new(path) && volume.is_none_or(|id| row.volume == id));
    if rows.len() != 4 || !known(Role::System, SYSTEM, None) || !known(Role::Recycle, RECYCLE, None) ||
        !known(Role::File, SOURCE, Some(from.setting.volume)) || !known(Role::File, TARGET, Some(to.setting.volume)) {
        return Err(invalid("different original cross-volume fixture catalog"));
    }

    let (birth, parent) = file_create::committed_origin(db, &catalog, request.file)?;
    if parent != DirectoryParent::Root || birth.volume != from.setting.volume || birth.name != FILE {
        return Err(invalid("different original fixture FILE birth"));
    }
    let original = Place::origin(&birth, parent);
    let selected = Place::current(db, &catalog, request.file)?;
    Place::require_no_rename(db, request.file)?;
    match request.phase {
        Phase::Source if selected == original => {}
        Phase::Target { move_op } => {
            let receipt = from.fixture_confirmed_move(to, db, &catalog, &original, move_op)?;
            if receipt.from != original || receipt.to != selected {
                return Err(invalid("current fixture FILE is not the exact completed move"));
            }
        }
        Phase::Source => return Err(invalid("fixture FILE is not at its original source")),
    }
    Ok(Cut { catalog, birth, original, selected })
}

fn version(file: &Current<'_, '_, '_>, place: &Place, node: NodeId, byte: u8) -> io::Result<VersionProof> {
    if file.native_placement() != place || file.size() != BYTES as u64 {
        return Err(invalid("wrong fixture placement or historical length"));
    }
    let mut bytes = [0; BYTES];
    let mut sink = &mut bytes[..];
    file.range_to(0, BYTES as u64, &mut sink)?;
    if !sink.is_empty() || bytes.iter().any(|&found| found != byte) {
        return Err(invalid("historical fixture bytes differ from the known original"));
    }
    let mut sha = Sha256::new();
    sha.update(&bytes)?;
    Ok(VersionProof {
        node, save: file.working_original().ok_or_else(|| invalid("missing exact historical SAVE"))?,
        revision: file.working_revision(), bytes: BYTES as u64, sha256: sha.finish(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Recorded {
    Pending,
    Complete,
}

fn row_state(status: u8, guard_absent: u8) -> io::Result<Recorded> {
    match (status, guard_absent) {
        (0, 0) => Ok(Recorded::Pending),
        (1, 1) => Ok(Recorded::Complete),
        _ => Err(invalid("source retirement is not positively recorded")),
    }
}

fn recorded_retirement(db: &Connection, place: &Place) -> io::Result<Recorded> {
    super::retirement::applied_target(db, place)?;
    let q = db.prepare(c"SELECT status,guard IS NULL FROM main.file_transfer_source WHERE transfer_op=?1")?;
    q.bind_bytes(1, &place.op.0)?;
    if !work_record::start_one(&q)? {
        return Err(invalid("missing fixture source retirement"));
    }
    let status = row_state(q.byte(0)?, q.byte(1)?)?;
    q.done()?;
    Ok(status)
}

fn require_vacant(names: [bool; 3]) -> io::Result<()> {
    if names.contains(&true) {
        return Err(invalid("recorded retirement still has an original physical name"));
    }
    Ok(())
}

pub(crate) fn observe(request: &FixtureRequest<'_>) -> io::Result<FixtureProof> {
    let [a, b] = request.nodes;
    if a == b || [a.0, b.0, request.pool.0, request.file.0, request.from.0, request.to.0].contains(&[0; 16])
        || request.from == request.to || matches!(request.phase, Phase::Target { move_op } if move_op.0 == [0; 16]) {
        return Err(invalid("missing or duplicate fixture identity"));
    }

    let source = InitialReader::open(Path::new(SYSTEM), request.pool)?;
    let session = SmbSession::fixture_authenticated(&source, request.login)?;
    let from = session.connect(request.from)?;
    let to = session.connect(request.to)?;
    let current = |db: &Connection, c: Catalog| select(db, c, request, &from, &to);
    let cut = source.at(&current)?;
    let same = |db: &Connection, c: Catalog| -> io::Result<()> {
        let later = current(db, c)?;
        if later.catalog.rows() != cut.catalog.rows() ||
            (later.catalog.birth.op, later.catalog.birth.request) != (cut.catalog.birth.op, cut.catalog.birth.request) ||
            later.birth != cut.birth || later.selected != cut.selected {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(())
    };

    let recycle = RecycleRoot::at(&source, &cut.catalog)?;
    let old = OrdinaryRoot::for_body(&source, &cut.catalog, from.setting.volume)?;
    let new = OrdinaryRoot::for_body(&source, &cut.catalog, to.setting.volume)?;
    let roots = [&source.dir.root, recycle.root_for_read(), old.folder(), new.folder()];
    for (i, root) in roots.iter().enumerate() {
        for other in &roots[..i] {
            if root.entity.device == other.entity.device || root.same_mount(other)? {
                return Err(invalid("fixture roles are not on four independent filesystems"));
            }
        }
    }
    let (tree, root) = match request.phase {
        Phase::Source => (&from, &old),
        Phase::Target { .. } => (&to, &new),
    };
    // Hold both witnesses on the same already qualified F through the byte
    // reads and late original FILE checks, never a second name-based mount.
    let first = tree.fixture_version_at(root, FILE, a)?;
    let second = tree.fixture_version_at(root, FILE, b)?;
    let versions = [
        version(&first, &cut.selected, a, b'A')?,
        version(&second, &cut.selected, b, b'B')?,
    ];
    first.check_at_file(|| Ok(()))?;
    second.check_at_file(|| Ok(()))?;
    let media = || {
        recycle.require()?;
        old.require_streaming()?;
        new.require_streaming()
    };
    media()?;

    let retirement = match request.phase {
        Phase::Source => {
            source.at(&same)?;
            Retirement::NotMoved
        }
        Phase::Target { move_op } => {
            let state = |db: &Connection, c: Catalog| {
                same(db, c)?;
                recorded_retirement(db, &cut.selected)
            };
            match source.at(&state)? {
                Recorded::Pending => Retirement::Pending,
                Recorded::Complete => {
                    let folder = old.folder();
                    let names = folder.names()?;
                    require_vacant([
                        folder.exact_presence(&names, &cut.original.leaf()?)?,
                        folder.exact_presence(&names, &cut.birth.witness()?)?,
                        folder.exact_presence(&names, &Birth::retirement_name(move_op)?)?,
                    ])?;
                    media()?;
                    if source.at(&state)? != Recorded::Complete {
                        return Err(io::ErrorKind::WouldBlock.into());
                    }
                    Retirement::Retired
                }
            }
        }
    };
    Ok(FixtureProof {
        file: cut.selected.file,
        volume: cut.selected.volume,
        position: cut.selected.op,
        generation: cut.selected.generation,
        versions,
        retirement,
    })
}

#[test]
fn cross_f_fixture_pending_is_not_retired_and_any_remaining_name_refuses() {
    assert_eq!(row_state(0, 0).unwrap(), Recorded::Pending);
    assert_eq!(row_state(1, 1).unwrap(), Recorded::Complete);
    for pair in [(0, 1), (1, 0), (2, 0), (2, 1)] {
        assert!(row_state(pair.0, pair.1).is_err());
    }
    assert!(require_vacant([false; 3]).is_ok());
    for i in 0..3 {
        let mut remaining = [false; 3];
        remaining[i] = true;
        assert!(require_vacant(remaining).is_err());
    }
}
