//! Dedicated original-maintenance control, not an externally callable service.
//! Every receive is from the exact held peer of this invocation,
//! with `SCM_CREDENTIALS` and exactly the descriptors fixed by this stage.
use super::{Worker, bad};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    net::{
        AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
        SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags, SocketType, UCred,
        recvmsg, sendmsg, socketpair, sockopt,
    },
};
use std::{
    fs::File,
    io::{self, IoSlice, IoSliceMut},
    mem::MaybeUninit,
};
const MAGIC: &[u8; 8] = b"ZXSAMW1\0";
const WIRE: usize = 8 + 1 + 32 + 32 + 65;
pub(crate) type Payload = [u8; 65];

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    ImportStep = 45,
    InitData = 46,
    ImportBatch = 47,
    VerifyContainer = 48,
}

pub(crate) struct Channel {
    fd: File,
    peer: UCred,
    scope: [u8; 32],
}
pub(crate) fn pair() -> io::Result<(File, File)> {
    let (a, b) = socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )?;
    for f in [&a, &b] {
        sockopt::set_socket_passcred(f, true)?;
    }
    Ok((File::from(a), File::from(b)))
}
pub(crate) fn seal_import_ticket(bytes: &[u8]) -> io::Result<File> {
    use std::io::Write;
    seal_import_order(|file| file.write_all(bytes))
}
pub(crate) fn seal_import_order(
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<File> {
    use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, memfd_create};
    use std::io::Seek;
    let mut ticket = File::from(memfd_create(
        c"helper-fixed-import",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?);
    fill(&mut ticket)?;
    let seals = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
    fcntl_add_seals(&ticket, seals)?;
    if !fcntl_get_seals(&ticket)?.contains(seals) {
        return Err(bad("incomplete fixed import ticket"));
    }
    ticket.rewind()?;
    Ok(ticket)
}

impl Channel {
    pub(crate) fn from_owned_child_session(
        fd: File,
        peer: UCred,
        scope: [u8; 32],
    ) -> io::Result<Self> {
        // Caller verifies actual supervising spawn, user/subject and startup
        // challenge before using this transport. SO_PEERCRED alone is only
        // the original socketpair creator, not this packet's sender.
        if !sockopt::socket_passcred(&fd)?
            || sockopt::socket_domain(&fd)? != AddressFamily::UNIX
            || sockopt::socket_type(&fd)? != SocketType::SEQPACKET
            || scope == [0; 32]
        {
            return Err(bad("not the dedicated offline maintenance peer"));
        }
        Ok(Self { fd, peer, scope })
    }
    fn send_inner(
        &self,
        kind: Kind,
        digest: &[u8; 32],
        body: &Payload,
        fds: &[&File],
    ) -> io::Result<()> {
        if fds.len() > 1 {
            return Err(bad("invalid fixed maintenance descriptor count"));
        }
        let tag = [kind as u8];
        let parts = [
            IoSlice::new(MAGIC),
            IoSlice::new(&tag),
            IoSlice::new(&self.scope),
            IoSlice::new(digest),
            IoSlice::new(body),
        ];
        let refs = fds.iter().map(|f| f.as_fd()).collect::<Vec<_>>();
        let mut ancillary = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = SendAncillaryBuffer::new(&mut ancillary);
        if !refs.is_empty() && !control.push(SendAncillaryMessage::ScmRights(&refs)) {
            return Err(bad("cannot attach the original maintenance descriptor"));
        }
        if sendmsg(
            &self.fd,
            &parts,
            &mut control,
            SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
        )? != WIRE
        {
            return Err(bad("incomplete maintenance message"));
        }
        Ok(())
    }
    pub(crate) fn send(&self, kind: Kind, digest: &[u8; 32], body: &Payload) -> io::Result<()> {
        self.send_inner(kind, digest, body, &[])
    }
    pub(crate) fn send_original(
        &self,
        kind: Kind,
        digest: &[u8; 32],
        body: &Payload,
        original: &File,
    ) -> io::Result<()> {
        self.send_inner(kind, digest, body, &[original])
    }
    fn receive<const K: usize>(
        &self,
        kind: Kind,
        digest: &[u8; 32],
    ) -> io::Result<(Payload, [File; K])> {
        let mut wire = [0; WIRE];
        let mut space =
            [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2), ScmCredentials(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let msg = recvmsg(
            &self.fd,
            &mut [IoSliceMut::new(&mut wire)],
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC | RecvFlags::DONTWAIT,
        )?;
        let mut credentials = None;
        let mut rights = Vec::<OwnedFd>::new();
        let mut submitted = false;
        for c in ancillary.drain() {
            match c {
                RecvAncillaryMessage::ScmCredentials(who) if credentials.is_none() => {
                    credentials = Some(who);
                }
                RecvAncillaryMessage::ScmRights(fds) if !submitted => {
                    submitted = true;
                    rights.extend(fds);
                }
                _ => return Err(bad("wrong maintenance ancillary envelope")),
            }
        }
        let who = credentials.is_some_and(|v| {
            v.pid == self.peer.pid && v.uid == self.peer.uid && v.gid == self.peer.gid
        });
        if !who
            || msg.bytes != WIRE
            || msg
                .flags
                .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC)
            || &wire[..8] != MAGIC
            || wire[8] != kind as u8
            || wire[9..41] != self.scope
            || wire[41..73] != *digest
        {
            return Err(bad("wrong, closed or stale offline maintenance peer"));
        }
        let fds: [OwnedFd; K] = rights
            .try_into()
            .map_err(|_| bad("wrong maintenance descriptor count"))?;
        Ok((
            wire[73..].try_into().expect("fixed payload"),
            fds.map(File::from),
        ))
    }
    pub(crate) fn wait_import(&self, digest: &[u8; 32], worker: &Worker) -> io::Result<Payload> {
        self.wait_offline(Kind::ImportStep, digest, worker)
    }
    pub(crate) fn wait_offline(
        &self,
        kind: Kind,
        digest: &[u8; 32],
        worker: &Worker,
    ) -> io::Result<Payload> {
        if self.peer.pid != worker.pid {
            return Err(bad(
                "offline maintenance channel is not the original worker's",
            ));
        }
        wait_packet(
            || self.receive::<0>(kind, digest).map(|(payload, _)| payload),
            || poll_ready(self.fd.as_fd(), worker.wait_descriptor()),
        )
    }
    pub(crate) fn eof(&self) -> io::Result<()> {
        retry_interrupted(|| {
            let mut buffer = [0];
            let mut ctl =
                [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1), ScmCredentials(1))];
            let mut extras = RecvAncillaryBuffer::new(&mut ctl);
            let msg = recvmsg(
                &self.fd,
                &mut [IoSliceMut::new(&mut buffer)],
                &mut extras,
                RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
            )?;
            if msg.bytes != 0
                || msg
                    .flags
                    .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC)
                || extras.drain().next().is_some()
            {
                return Err(bad(
                    "offline maintenance worker did not close its exact result channel",
                ));
            }
            Ok(())
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Readiness {
    Input,
    WorkerExited,
    Unusable,
}
impl Readiness {
    fn from_events(socket: PollFlags, worker: PollFlags) -> io::Result<Self> {
        if (socket | worker).intersects(PollFlags::ERR | PollFlags::NVAL) {
            Ok(Self::Unusable)
        } else if worker.intersects(PollFlags::IN | PollFlags::HUP) {
            Ok(Self::WorkerExited)
        } else if socket.contains(PollFlags::HUP) {
            Ok(Self::Unusable)
        } else if socket.contains(PollFlags::IN) {
            Ok(Self::Input)
        } else {
            Err(bad(
                "no readiness for the original maintenance channel or worker",
            ))
        }
    }

    fn require_waitable(self) -> io::Result<()> {
        match self {
            Self::Input => Ok(()),
            Self::WorkerExited => Err(bad(
                "the original maintenance worker ended before the next step",
            )),
            Self::Unusable => Err(bad(
                "the original maintenance channel or worker became unusable",
            )),
        }
    }
}

fn poll_ready(socket: BorrowedFd<'_>, worker: BorrowedFd<'_>) -> io::Result<Readiness> {
    let mut held = [
        PollFd::from_borrowed_fd(socket, PollFlags::IN),
        PollFd::from_borrowed_fd(worker, PollFlags::IN),
    ];
    poll(&mut held, None)?;
    Readiness::from_events(held[0].revents(), held[1].revents())
}

fn retry_interrupted<T>(mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match call() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

fn wait_packet(
    mut receive: impl FnMut() -> io::Result<Payload>,
    mut wait: impl FnMut() -> io::Result<Readiness>,
) -> io::Result<Payload> {
    let mut readiness = Readiness::Input;
    loop {
        // Read the exact packet even if the socket hung up or the original
        // pidfd was reported ready. A queued terminal takes precedence;
        // callers still require the exact child's exit and channel EOF.
        match retry_interrupted(&mut receive) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                readiness.require_waitable()?;
                readiness = retry_interrupted(&mut wait)?;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::process::{Uid, getgid, getpid, getuid};
    #[test]
    fn a_complete_scope_operation_sender_and_stage_are_required() {
        let (a, b) = pair().unwrap();
        let c = UCred {
            uid: getuid(),
            gid: getgid(),
            pid: getpid(),
        };
        let root = Channel::from_owned_child_session(a, c, [8; 32]).unwrap();
        let worker = Channel::from_owned_child_session(b, c, [8; 32]).unwrap();
        let data = [5; 65];
        let digest = [4; 32];
        root.send(Kind::ImportStep, &digest, &data).unwrap();
        assert_eq!(
            worker.receive::<0>(Kind::ImportStep, &digest).unwrap().0,
            data
        );
        root.send(Kind::ImportBatch, &digest, &data).unwrap();
        assert!(worker.receive::<0>(Kind::ImportStep, &digest).is_err());
        root.send(Kind::ImportBatch, &digest, &data).unwrap();
        assert_eq!(
            worker.receive::<0>(Kind::ImportBatch, &digest).unwrap().0,
            data
        );
        root.send(Kind::InitData, &digest, &data).unwrap();
        assert!(worker.receive::<0>(Kind::ImportStep, &digest).is_err());
        root.send(Kind::ImportStep, &[7; 32], &data).unwrap();
        assert!(worker.receive::<0>(Kind::ImportStep, &digest).is_err());
        let mut wrong = c;
        wrong.uid = Uid::from_raw(u32::from(c.uid.is_root()));
        let worker = Channel::from_owned_child_session(worker.fd, wrong, [8; 32]).unwrap();
        root.send(Kind::ImportStep, &digest, &data).unwrap();
        assert!(worker.receive::<0>(Kind::ImportStep, &digest).is_err());
        drop(root);
        worker.eof().unwrap();
    }

    #[test]
    fn queued_terminal_wins_after_simultaneous_hangup_and_exit_despite_interruptions() {
        let terminal = [0x42; 65];
        let mut packets = [
            Err(io::ErrorKind::WouldBlock.into()),
            Err(io::ErrorKind::Interrupted.into()),
            Ok(terminal),
        ]
        .into_iter();
        let mut events = [
            Err(io::ErrorKind::Interrupted.into()),
            Readiness::from_events(PollFlags::IN | PollFlags::HUP, PollFlags::IN),
        ]
        .into_iter();
        assert_eq!(
            wait_packet(|| packets.next().unwrap(), || events.next().unwrap()).unwrap(),
            terminal
        );
        assert!(packets.next().is_none() && events.next().is_none());
    }

    #[test]
    fn terminal_wake_with_no_packet_or_invalid_envelope_cannot_spin_or_succeed() {
        for (socket, worker) in [
            (PollFlags::empty(), PollFlags::IN),
            (PollFlags::HUP, PollFlags::empty()),
            (PollFlags::NVAL, PollFlags::empty()),
        ] {
            let mut wake = Some(Readiness::from_events(socket, worker));
            assert!(
                wait_packet(
                    || Err(io::ErrorKind::WouldBlock.into()),
                    || wake.take().unwrap(),
                )
                .is_err()
            );
            assert!(wake.is_none());
        }

        let mut packets = [
            Err(io::ErrorKind::WouldBlock.into()),
            Err(bad("invalid exact packet")),
        ]
        .into_iter();
        let err =
            wait_packet(|| packets.next().unwrap(), || Ok(Readiness::WorkerExited)).unwrap_err();
        assert_eq!(err.to_string(), "invalid exact packet");
    }

    #[test]
    fn queued_socket_input_on_held_synthetic_descriptors_is_not_eof() {
        let (a, b) = pair().unwrap();
        let credentials = UCred {
            uid: getuid(),
            gid: getgid(),
            pid: getpid(),
        };
        let receiver = Channel::from_owned_child_session(a, credentials, [8; 32]).unwrap();
        let sender = Channel::from_owned_child_session(b, credentials, [8; 32]).unwrap();
        let (ended, signal) = rustix::pipe::pipe().unwrap();
        drop(signal); // Synthetic readiness FD; no child or actual process is created.
        sender
            .send(Kind::VerifyContainer, &[3; 32], &[4; 65])
            .unwrap();
        drop(sender);
        assert_eq!(
            poll_ready(receiver.fd.as_fd(), ended.as_fd()).unwrap(),
            Readiness::WorkerExited
        );
        assert!(receiver.eof().is_err()); // Reject and drain the surplus exact packet.
        receiver.eof().unwrap();
    }
}
