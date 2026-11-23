//! One direct one-shot low-to-low handoff of the salted Web material after
//! joint derivation. No external command. The driver hands exactly
//! one end to each authenticated child and drops both of the parent's copies
//! before beginning hidden input. A payload is not permission to reserve SQL.
use super::{
    bad,
    transport::{Channel, Kind, Payload},
};
use crate::{
    maintenance::diagnostic_worker,
    sqlite_install::pair::{
        Id,
        terminal::{JointWeb, protected_process},
        web_secret::PreparedWeb,
    },
};
use helper_linux_evidence::first_smb::require_rust_v2_public_prefix;
use rusqlite::Transaction;
use rustix::{
    net::UCred,
    process::{getgid, getpid, getuid},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io, thread,
    time::{Duration, Instant},
};

fn rejected() -> io::Error {
    bad("different original joint credential peer, scope or material")
}
/// Private invocation scope, not the persistent public SETUP/RESET digest.
/// All coordinates come from the actual frozen proposal and identified
/// original children, never from an arbitrary input channel or receipt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::sqlite_install::pair) struct Epoch {
    pub(in crate::sqlite_install::pair) op: Id,
    pub(in crate::sqlite_install::pair) generation: i64,
    pub(in crate::sqlite_install::pair) record: Id,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::sqlite_install::pair) struct Binding {
    action: [u8; 32],
    transcript: [u8; 32],
    body: Payload,
    prefix: [u8; 224],
    target: Epoch,
    previous: Option<Epoch>,
}
impl Binding {
    pub(in crate::sqlite_install::pair) fn new(
        nonce: [u8; 32],
        action: [u8; 32],
        account: Id,
        target: Epoch,
        previous: Option<Epoch>,
        prefix: [u8; 224],
    ) -> io::Result<Self> {
        require_rust_v2_public_prefix(&prefix)?;
        let Epoch {
            op,
            generation,
            record,
        } = target;
        let monotonic = match previous {
            None => generation == 1,
            Some(old) => {
                old.generation > 0
                    && old.generation.checked_add(1) == Some(generation)
                    && old.op != op
                    && old.record != record
                    && old.op != [0; 16]
                    && old.record != [0; 16]
                    && if old.generation == 1 {
                        old.op == prefix[56..72]
                    } else {
                        !prefix[24..104].as_chunks::<16>().0.contains(&old.op)
                    }
            }
        };
        if [nonce, action].contains(&[0; 32])
            || [account, op, record].contains(&[0; 16])
            || !monotonic
            || prefix[104..120] != account
            || prefix[120..136] != op
            || prefix[136..152] != record
            || prefix[152..160] != generation.to_be_bytes()
        {
            return Err(rejected());
        }
        let mut body = [0; 65];
        body[0] = 1;
        body[1..17].copy_from_slice(&account);
        body[17..33].copy_from_slice(&op);
        body[33..49].copy_from_slice(&record);
        body[49..57].copy_from_slice(&generation.to_be_bytes());
        let mut h = Sha256::new();
        h.update(b"ZXS-HELPER/JOINT-WEB-MATERIAL-1\0");
        h.update(nonce);
        h.update(action);
        h.update(body);
        h.update(prefix);
        match previous {
            None => h.update([0]),
            Some(old) => {
                h.update([1]);
                h.update(old.op);
                h.update(old.record);
                h.update(old.generation.to_be_bytes());
            }
        }
        let transcript = h.finalize().into();
        Ok(Self {
            action,
            transcript,
            body,
            prefix,
            target,
            previous,
        })
    }
    pub(in crate::sqlite_install::pair) fn prefix(&self) -> &[u8; 224] {
        &self.prefix
    }
    pub(in crate::sqlite_install::pair) fn require_same(&self, expected: &Self) -> io::Result<()> {
        if self != expected {
            return Err(rejected());
        }
        Ok(())
    }
}

struct End {
    channel: Channel,
    binding: Binding,
    peer: UCred,
}
impl End {
    fn original(fd: File, peer: UCred, binding: Binding) -> io::Result<Self> {
        if peer.pid == getpid()
            || peer.uid.is_root()
            || peer.gid.is_root()
            || peer.uid != getuid()
            || peer.gid != getgid()
        {
            return Err(rejected());
        }
        let channel = Channel::from_owned_child_session(fd, peer, binding.transcript)?;
        Ok(Self {
            channel,
            binding,
            peer,
        })
    }
}

#[must_use]
pub(super) struct Writer(End);
impl Writer {
    /// Call after the actual supervising local channel identifies the other
    /// original born low PID/UID/GID. Sender still needs exact reader proof.
    pub(super) fn connect(
        fd: File,
        peer: UCred,
        binding: Binding,
        until: Instant,
        alive: impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        diagnostic_worker(peer.uid.as_raw(), peer.gid.as_raw())?;
        let end = End::original(fd, peer, binding)?;
        end.await_reader(until, alive)?;
        Ok(Self(end))
    }
    // No arbitrary byte or FD sender. This consumes the one exact
    // successfully joint-derived Web result and the write endpoint.
    pub(super) fn deliver(
        self,
        result: JointWeb,
        alive: impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        alive()?;
        let owned = result.seal_for_peer(&self.0.binding)?;
        self.0.send_file(&owned, alive)
    }
}

#[must_use]
pub(super) struct Reader(End);

/// Still tied to the authorized operation and actual provider prefix.
/// The original SQL receiver compares its own new plan/receipt again; no
/// unbound `PreparedWeb` escapes merely on successful IPC.
#[must_use]
pub(in crate::sqlite_install::pair) struct Received {
    binding: Binding,
    web: PreparedWeb,
}
pub(in crate::sqlite_install::pair) struct ReceivePacking(());
#[must_use]
pub(in crate::sqlite_install::pair) struct StagedWeb {
    binding: Binding,
    web: PreparedWeb,
}
impl Received {
    #[cfg(test)]
    pub(in crate::sqlite_install::pair) fn synthetic(binding: Binding, pattern: u8) -> Self {
        Self {
            binding,
            web: PreparedWeb::synthetic([pattern; 16], [pattern.wrapping_add(1); 32]),
        }
    }
    /// Consume only inside the actual low original T1. Its caller still
    /// proves the prior joint head, exact plan and staged vault independently:
    /// this typed material is not a writable VFS permit or a transaction.
    pub(in crate::sqlite_install::pair) fn stage(
        self,
        expected: &Binding,
        tx: &Transaction<'_>,
    ) -> io::Result<StagedWeb> {
        self.binding.require_same(expected)?;
        let next = self.binding.target;
        let n = match self.binding.previous {
            None => self.web.insert(tx, &next.op),
            Some(old) => self
                .web
                .reset(tx, &next.op, &old.op, next.generation, old.generation),
        }
        .map_err(|_| rejected())?;
        if n != 1 {
            return Err(rejected());
        }
        let staged = StagedWeb {
            binding: self.binding,
            web: self.web,
        };
        staged.require_stored(expected, tx)?;
        Ok(staged)
    }
}
impl StagedWeb {
    /// The opaque original tuple can be compared again before T2; it never
    /// exports an ordinary `PreparedWeb` or caller-selected SQL coordinates.
    pub(in crate::sqlite_install::pair) fn require_stored(
        &self,
        expected: &Binding,
        tx: &Transaction<'_>,
    ) -> io::Result<()> {
        self.binding.require_same(expected)?;
        self.web
            .require_stored(tx, &self.binding.target.op, self.binding.target.generation)
    }
}

impl Reader {
    /// The actual permanently lowered SQL receiver locks its memory and
    /// prevents dump/trace before offering to receive any verifier bytes.
    pub(super) fn connect(
        fd: File,
        peer: UCred,
        binding: Binding,
        alive: impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        protected_process(peer.uid.as_raw(), peer.gid.as_raw())?;
        let end = End::original(fd, peer, binding)?;
        end.ready(alive)?;
        Ok(Self(end))
    }
    /// Later used only after the supervising supervisor has independently
    /// reaped the real writer and authenticated the same staged vault.
    /// A truncated, duplicated, stale or altered message yields nothing.
    pub(super) fn receive(
        self,
        until: Instant,
        alive: impl Fn() -> io::Result<()>,
    ) -> io::Result<Received> {
        self.0.receive(until, alive)
    }
}

impl End {
    fn ready(&self, alive: impl Fn() -> io::Result<()>) -> io::Result<()> {
        alive()?;
        self.channel.send(
            Kind::JointWebReceiverReady,
            &self.binding.action,
            &self.binding.body,
        )?;
        self.channel.finish_writing()?;
        alive()
    }
    fn await_reader(&self, until: Instant, alive: impl Fn() -> io::Result<()>) -> io::Result<()> {
        let (which, []) = self.channel.wait_checked::<0>(
            Kind::JointWebReceiverReady,
            &self.binding.action,
            until,
            &alive,
        )?;
        if which != self.binding.body {
            return Err(rejected());
        }
        self.exact_eof(until, alive)
    }
    fn exact_eof(&self, until: Instant, alive: impl Fn() -> io::Result<()>) -> io::Result<()> {
        loop {
            alive()?;
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the exact joint low peer did not end its message",
                ));
            }
            match self.channel.eof() {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
    fn send_file(self, file: &File, alive: impl Fn() -> io::Result<()>) -> io::Result<()> {
        alive()?;
        self.channel.send_original(
            Kind::JointWebMaterial,
            &self.binding.action,
            &self.binding.body,
            file,
        )?;
        self.channel.finish_writing()?;
        alive()
    }
    fn receive(self, until: Instant, alive: impl Fn() -> io::Result<()>) -> io::Result<Received> {
        let (which, [file]) = self.channel.wait_checked::<1>(
            Kind::JointWebMaterial,
            &self.binding.action,
            until,
            &alive,
        )?;
        if which != self.binding.body {
            return Err(rejected());
        }
        self.exact_eof(until, &alive)?;
        let material = PreparedWeb::from_joint_peer(
            &file,
            self.peer.uid.as_raw(),
            self.peer.gid.as_raw(),
            ReceivePacking(()),
        )?;
        alive()?;
        Ok(Received {
            binding: self.binding,
            web: material,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::{
        fs::{Mode, fchmod},
        io::fcntl_dupfd_cloexec,
        process::{Pid, getppid},
    };
    use std::process::{Command, Stdio};
    const CASE: &str = "HELPER_TEST_JOINT_WEB_CASE";

    fn fixture<const N: usize>(field: &str) -> [u8; N] {
        let entries = include_str!("../../../../tests/sqlite/shared-account-public-v1.txt");
        let text = entries
            .lines()
            .find_map(|r| {
                r.split_once('=')
                    .filter(|(k, _)| *k == field)
                    .map(|(_, v)| v)
            })
            .unwrap();
        assert_eq!(text.len(), 2 * N);
        std::array::from_fn(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
    }
    fn old() -> Epoch {
        Epoch {
            op: fixture("setup"),
            generation: 1,
            record: fixture("first_record"),
        }
    }
    fn target() -> Epoch {
        Epoch {
            op: fixture("new_op"),
            generation: 2,
            record: fixture("new_record"),
        }
    }
    fn binding() -> Binding {
        Binding::new(
            [0x17; 32],
            fixture("reset_sha"),
            fixture("account"),
            target(),
            Some(old()),
            fixture("new_prefix"),
        )
        .unwrap()
    }
    fn other() -> Binding {
        Binding::new(
            [0x17; 32],
            fixture("setup_sha"),
            fixture("account"),
            old(),
            None,
            fixture("first_prefix"),
        )
        .unwrap()
    }
    fn tuple(tx: &Transaction<'_>) -> (Vec<u8>, i64, Vec<u8>, Vec<u8>) {
        tx.query_row(
            "SELECT credential_op,generation,salt,verifier FROM web_password",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }
    fn simple() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE web_password(slot INTEGER PRIMARY KEY,credential_op BLOB,generation INTEGER,salt BLOB,verifier BLOB);").unwrap();
        conn
    }
    fn peer(pid: Pid) -> UCred {
        UCred {
            pid,
            uid: getuid(),
            gid: getgid(),
        }
    }
    fn exact_material() -> File {
        JointWeb::synthetic(binding())
            .seal_for_peer(&binding())
            .unwrap()
    }
    fn case_in_child(case: &str) {
        let socket = File::from(fcntl_dupfd_cloexec(io::stdin(), 3).unwrap());
        // Unit test for genuine different process SCM only. No privilege,
        // tracing change, credential, database on disk or host runtime.
        let end = End::original(socket, peer(getppid().unwrap()), binding()).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        if case == "wrong_ready" {
            assert!(end.await_reader(until, || Ok(())).is_err());
            println!("joint-test-reader-rejected");
            return;
        }
        end.await_reader(until, || Ok(())).unwrap();
        match case {
            "exact" => {
                let material = exact_material();
                end.channel
                    .send_original(
                        Kind::JointWebMaterial,
                        &end.binding.action,
                        &end.binding.body,
                        &material,
                    )
                    .unwrap();
                // Real half-close occurs after the reader may have received
                // the packet. EOF must be polled with the original deadline,
                // not rejected just because the scheduler ran the reader first.
                thread::sleep(Duration::from_millis(25));
                end.channel.finish_writing().unwrap();
            }
            "wrong_actual_pid" => end.send_file(&exact_material(), || Ok(())).unwrap(),
            "wrong_record" => {
                let mut changed = end.binding.body;
                changed[33] ^= 1;
                end.channel
                    .send_original(
                        Kind::JointWebMaterial,
                        &end.binding.action,
                        &changed,
                        &exact_material(),
                    )
                    .unwrap();
                end.channel.finish_writing().unwrap();
            }
            "no_right" => {
                end.channel
                    .send(
                        Kind::JointWebMaterial,
                        &end.binding.action,
                        &end.binding.body,
                    )
                    .unwrap();
                end.channel.finish_writing().unwrap();
            }
            "two_packets" => {
                let file = exact_material();
                for _ in 0..2 {
                    end.channel
                        .send_original(
                            Kind::JointWebMaterial,
                            &end.binding.action,
                            &end.binding.body,
                            &file,
                        )
                        .unwrap();
                }
                end.channel.finish_writing().unwrap();
            }
            "short" => {
                let file = crate::sqlite_install::pair::domain::sealed(&[0x13; 47]).unwrap();
                fchmod(&file, Mode::RUSR).unwrap();
                end.send_file(&file, || Ok(())).unwrap();
            }
            _ => panic!("unknown synthetic mode"),
        }
        println!("joint-test-sent");
    }
    #[test]
    fn joint_account_material_requires_the_other_actual_process_and_one_closed_exact_web_result() {
        if let Ok(case) = std::env::var(CASE) {
            case_in_child(&case);
            return;
        }
        assert!(
            !getuid().is_root(),
            "use the independent unprivileged fixture"
        );
        for case in [
            "exact",
            "wrong_actual_pid",
            "wrong_record",
            "no_right",
            "two_packets",
            "short",
            "wrong_ready",
        ] {
            let (connected, original) =
                crate::sqlite_install::pair::native_run::transport::pair().unwrap();
            let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",
                "sqlite_install::pair::native_run::joint_material::tests::joint_account_material_requires_the_other_actual_process_and_one_closed_exact_web_result",
                "--nocapture"]).stdin(Stdio::from(original)).env(CASE,case).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
            let wanted = if case == "wrong_actual_pid" {
                Pid::from_raw(pid.as_raw_pid() + 100_000).unwrap()
            } else {
                pid
            };
            let end = End::original(connected, peer(wanted), binding()).unwrap();
            if case == "wrong_ready" {
                // Even with the correct reader's genuine SCM credentials,
                // a different Ready tuple may not obtain any material.
                let mut changed = end.binding.body;
                changed[49] ^= 1;
                end.channel
                    .send(Kind::JointWebReceiverReady, &end.binding.action, &changed)
                    .unwrap();
                end.channel.finish_writing().unwrap();
                end.exact_eof(until, || Ok(())).unwrap();
            } else {
                end.ready(|| Ok(())).unwrap();
                let material = end.receive(until, || Ok(()));
                if case == "exact" {
                    let material = material.unwrap();
                    let mut c = simple();
                    let tx = c.transaction().unwrap();
                    assert_eq!(
                        PreparedWeb::synthetic([1; 16], [2; 32])
                            .insert(&tx, &old().op)
                            .unwrap(),
                        1
                    );
                    let staged = material.stage(&binding(), &tx).unwrap();
                    staged.require_stored(&binding(), &tx).unwrap();
                    assert_eq!(
                        tuple(&tx),
                        (target().op.to_vec(), 2, vec![0x31; 16], vec![0x32; 32])
                    );
                    tx.execute("UPDATE web_password SET verifier=zeroblob(32)", [])
                        .unwrap();
                    assert!(staged.require_stored(&binding(), &tx).is_err());
                } else {
                    assert!(material.is_err(), "{case} accepted a wrong low material");
                }
            }
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "synthetic child {case} did not finish correctly: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let observed = String::from_utf8_lossy(&result.stdout);
            assert!(observed.contains(if case == "wrong_ready" {
                "joint-test-reader-rejected"
            } else {
                "joint-test-sent"
            }));
        }
    }
    #[test]
    fn joint_account_material_receipt_binds_the_reviewed_public_epoch_without_password_bytes() {
        let b = binding();
        assert_eq!(&b.body[1..17], &fixture::<16>("account"));
        assert_eq!(&b.body[17..33], &fixture::<16>("new_op"));
        assert_eq!(&b.body[33..49], &fixture::<16>("new_record"));
        assert_eq!(&b.body[49..57], &2i64.to_be_bytes());
        assert_eq!(&b.body[57..], &[0; 8]);
        assert_eq!(b.action, fixture::<32>("reset_sha"));
        let other = Binding::new(
            [0x18; 32],
            b.action,
            fixture("account"),
            target(),
            Some(old()),
            fixture("new_prefix"),
        )
        .unwrap();
        assert_ne!(b.transcript, other.transcript);
        let fork = Binding::new(
            [0x17; 32],
            b.action,
            fixture("account"),
            target(),
            Some(Epoch {
                record: [8; 16],
                ..old()
            }),
            fixture("new_prefix"),
        )
        .unwrap();
        assert_ne!(b.transcript, fork.transcript);
        assert!(b.require_same(&other).is_err());
        let mut wrong = fixture::<224>("new_prefix");
        wrong[104] ^= 1;
        assert!(
            Binding::new(
                [1; 32],
                b.action,
                fixture("account"),
                target(),
                Some(old()),
                wrong
            )
            .is_err()
        );
        wrong = fixture("new_prefix");
        wrong[14] = b'1';
        assert!(
            Binding::new(
                [1; 32],
                b.action,
                fixture("account"),
                target(),
                Some(old()),
                wrong
            )
            .is_err()
        );
        assert!(
            Binding::new(
                [0; 32],
                b.action,
                fixture("account"),
                target(),
                Some(old()),
                fixture("new_prefix")
            )
            .is_err()
        );
        assert!(
            Binding::new(
                [1; 32],
                b.action,
                fixture("account"),
                target(),
                None,
                fixture("new_prefix")
            )
            .is_err()
        );
        assert!(
            Binding::new(
                [1; 32],
                b.action,
                fixture("account"),
                Epoch {
                    generation: 3,
                    ..target()
                },
                Some(old()),
                fixture("new_prefix")
            )
            .is_err()
        );
    }

    #[test]
    fn joint_account_material_cannot_relabel_either_typed_end_after_both_derivations() {
        let b = binding();
        let other = other();
        // Deliberately impossible pair: reject before even creating a memfd,
        // or binding the derived Web tuple to any SQLite statement.
        assert!(JointWeb::synthetic(b).seal_for_peer(&other).is_err());
        let material = || Received {
            binding: b,
            web: PreparedWeb::synthetic([0x31; 16], [0x32; 32]),
        };
        let mut c = simple();
        let tx = c.transaction().unwrap();
        PreparedWeb::synthetic([1; 16], [2; 32])
            .insert(&tx, &old().op)
            .unwrap();
        let before = tuple(&tx);
        assert!(material().stage(&other, &tx).is_err());
        assert_eq!(tuple(&tx), before);
        let wrong_prior = Binding::new(
            [0x17; 32],
            b.action,
            fixture("account"),
            target(),
            Some(Epoch {
                record: [81; 16],
                ..old()
            }),
            fixture("new_prefix"),
        )
        .unwrap();
        assert!(material().stage(&wrong_prior, &tx).is_err());
        assert_eq!(tuple(&tx), before);
        tx.execute("UPDATE web_password SET credential_op=zeroblob(16)", [])
            .unwrap();
        let mismatched = tuple(&tx);
        assert!(material().stage(&b, &tx).is_err());
        assert_eq!(tuple(&tx), mismatched);
        tx.execute("UPDATE web_password SET credential_op=?1", [&old().op[..]])
            .unwrap();
        let staged = material().stage(&b, &tx).unwrap();
        assert!(staged.require_stored(&other, &tx).is_err());
        staged.require_stored(&b, &tx).unwrap();
        assert_eq!(
            tuple(&tx),
            (target().op.to_vec(), 2, vec![0x31; 16], vec![0x32; 32])
        );
        assert!(material().stage(&b, &tx).is_err()); // even an isolated stale CAS cannot reapply
        assert_eq!(
            tuple(&tx),
            (target().op.to_vec(), 2, vec![0x31; 16], vec![0x32; 32])
        );
    }
}
