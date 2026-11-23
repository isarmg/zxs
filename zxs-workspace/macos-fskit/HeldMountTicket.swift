import CryptoKit
import Darwin
import Foundation

/// Two physically registered handles from the original app-group session. The
/// resource delegated to FSKit is JUST an empty ticket, never the directory,
/// credentials, a download URL, work payload or the private registration.
@available(macOS 27.0, *)
final class HeldMountTicket {
    static let group = "group.org.zipworkspace.desktop.remotefs"
    let token: String, digest: String, volume: UUID
    let directory: FileHandle, ticket: FileHandle
    let directoryID: WorkWitness, ticketID: WorkWitness
    let directoryURL: URL, url: URL

    init(token: String, digest: String, expectedVolume: UUID, directory: FileHandle, registered: WorkWitness,
         emptyTicket: FileHandle, issued: WorkWitness) throws {
        guard digest.utf8.count == 64, digest.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
              token.utf8.count == 32, token.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
              !token.utf8.allSatisfy({ $0 == 48 }), registered.volumeHex == issued.volumeHex,
              let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.group)
        else { throw workError(EINVAL) }
        self.token = token; self.digest = digest; volume = expectedVolume
        directoryID = registered; ticketID = issued
        func duplicate(_ h: FileHandle) throws -> FileHandle {
            let fd = fcntl(h.fileDescriptor, F_DUPFD_CLOEXEC, 0)
            guard fd >= 0 else { throw workError(errno) }
            return FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        }
        self.directory = try duplicate(directory); ticket = try duplicate(emptyTicket)
        directoryURL = base.appendingPathComponent("work-" + token, isDirectory: true)
        url = directoryURL.appendingPathComponent("ticket", isDirectory: false)
        try check()
    }

    func check() throws {
        try directoryID.check(directory.fileDescriptor, directory: true)
        try ticketID.check(ticket.fileDescriptor, directory: false)
        let ds = ("work-" + token).withCString { zxs_work_spelling(directory.fileDescriptor, $0) }
        let ts = "ticket".withCString { zxs_work_spelling(ticket.fileDescriptor, $0) }
        guard ds == 0, ts == 0 else { throw workError(ds == 0 ? ts : ds) }
        var namedTicket=stat(), original=stat()
        let p=directoryURL.withUnsafeFileSystemRepresentation { $0==nil ? EINVAL : zxs_work_path(directory.fileDescriptor,$0!) }
        let t=url.withUnsafeFileSystemRepresentation { $0==nil ? EINVAL : zxs_work_path(ticket.fileDescriptor,$0!) }
        guard p==0, t==0 else { throw workError(p==0 ? t : p) }
        guard fstatat(directory.fileDescriptor,"ticket",&namedTicket,AT_SYMLINK_NOFOLLOW)==0,
              fstat(ticket.fileDescriptor,&original)==0 else { throw workError(errno) }
        let flags=fcntl(ticket.fileDescriptor,F_GETFL)
        guard flags>=0, flags&O_ACCMODE==O_RDONLY, original.st_size==0,
              namedTicket.st_dev==original.st_dev, namedTicket.st_ino==original.st_ino else { throw workError(ESTALE) }
    }

    func matches(_ supplied: URL) -> Bool {
        guard supplied.isFileURL, supplied.host == nil || supplied.host == "",
              supplied.user == nil, supplied.password == nil, supplied.port == nil,
              supplied.query == nil, supplied.fragment == nil else { return false }
        return supplied.withUnsafeFileSystemRepresentation { actual in
            url.withUnsafeFileSystemRepresentation { expected in
                guard let actual, let expected else { return false }
                return strcmp(actual, expected) == 0
            }
        }
    }

    private var originalMark: Data {
        Data(("ZXS-FSKIT-2\n" + token + "\n" + digest + "\n" + volume.uuidString.lowercased() + "\n" +
              directoryID.volumeHex + "\n" + String(directoryID.inode) + ":" + String(directoryID.birthSec) +
              ":" + String(directoryID.birthNsec) + "\n" + String(ticketID.inode) + ":" + String(ticketID.birthSec) +
              ":" + String(ticketID.birthNsec) + "\n").utf8)
    }

    enum Step: String {
        case attempted = "host-attempt", loaded = "provider-loaded", verified = "host-verified", stop = "host-stop"
        case stopped = "provider-stopped", gone = "host-normal-gone", clean = "provider-local-clean"
    }

    // Monotonic original witnesses. Preexisting or torn records never prompt a
    // rewrite or reissue. The distribution must protect the group/one original
    // session against other publishers; 0700 is not same-user isolation.
    func mark(_ step: Step, proof: Data = Data()) throws {
        guard validProof(step,proof) else { throw workError(EINVAL) }
        try check()
        guard try !marked(step) else { throw workError(EEXIST) }
        let leaf = step == .attempted ? step.rawValue : "pre-" + step.rawValue
        let fd = leaf.withCString { openat(directory.fileDescriptor, $0,
            O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC, 0o600) }
        guard fd >= 0 else { throw workError(errno) }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        var st = stat()
        guard fstat(fd, &st) == 0 else { throw workError(errno) }
        let contents = bytes(step, st) + proof
        let terminal = step == .clean || step == .gone
        try withExtendedLifetime(handle) {
            try contents.withUnsafeBytes { buffer in
                var at = 0
                while at < buffer.count {
                    let n = pwrite(fd, buffer.baseAddress!.advanced(by: at), buffer.count-at, off_t(at))
                    if n < 0 && errno == EINTR { continue }
                    guard n > 0 else { throw workError(n == 0 ? EIO : errno) }
                    at += n
                }
            }
            guard fchmod(fd, 0o400) == 0, fsync(fd) == 0, fcntl(fd, F_FULLFSYNC) == 0 else { throw workError(errno) }
            if terminal {
                // The sole externally positive decision is the final exclusive rename. Stage,
                // independently read, sync and CLOSE the sole writer before that decision.
                var sealed=stat()
                guard fstat(fd,&sealed)==0 else { throw workError(errno) }
                guard sealed.st_dev==st.st_dev, sealed.st_ino==st.st_ino, sealed.st_size==contents.count,
                      sealed.st_nlink==1, sealed.st_flags==0, sealed.st_uid==geteuid(),
                      (sealed.st_mode & 0o177777)==0o100400, try readBytes(fd,contents.count)==contents
                else { throw workError(ESTALE) }
                st=sealed
                guard fsync(directory.fileDescriptor)==0, fcntl(fd,F_FULLFSYNC)==0 else { throw workError(errno) }
                try handle.close()
                try check()
            }
            if step != .attempted {
                var named = stat()
                guard fstatat(directory.fileDescriptor, leaf, &named, AT_SYMLINK_NOFOLLOW) == 0,
                      named.st_dev == st.st_dev, named.st_ino == st.st_ino,
                      !terminal || (named.st_size==st.st_size && named.st_ctimespec.tv_sec==st.st_ctimespec.tv_sec &&
                        named.st_ctimespec.tv_nsec==st.st_ctimespec.tv_nsec) else { throw workError(ESTALE) }
                let rc = leaf.withCString { old in step.rawValue.withCString { target in
                    renameatx_np(directory.fileDescriptor, old, directory.fileDescriptor, target, UInt32(RENAME_EXCL))
                } }
                guard rc == 0 else { throw workError(errno) }
            }
            if !terminal {
                guard fsync(directory.fileDescriptor) == 0, fcntl(fd, F_FULLFSYNC) == 0 else { throw workError(errno) }
            }
        }
        if !terminal {
            try handle.close() // Never retry a potentially reused numeric FD.
            guard try witness(step) == proof else { throw workError(ESTALE) }
        }
        // No throwing publisher action after a terminal rename. A consumer must still
        // persist/re-read *these exact* decisions; a fault or crash is never a guessed receipt.
    }

    private func readBytes(_ fd:Int32,_ length:Int) throws -> Data {
        var observed=[UInt8](repeating:0,count:length), at=0
        while at<length {
            let n=observed.withUnsafeMutableBytes { pread(fd,$0.baseAddress!.advanced(by:at),length-at,off_t(at)) }
            if n<0 && errno==EINTR { continue }
            guard n>0 else { throw workError(n==0 ? EIO : errno) }; at+=n
        }
        return Data(observed)
    }

    private func bytes(_ step: Step, _ st: stat) -> Data {
        originalMark + Data((step.rawValue + "\n" + String(st.st_ino) + ":" +
                             String(st.st_birthtimespec.tv_sec) + ":" + String(st.st_birthtimespec.tv_nsec) + "\n").utf8)
    }

    private static func fingerprint(_ contents:Data) -> Data {
        Data(SHA256.hash(data:contents).map { String(format:"%02x",$0) }.joined().utf8)
    }
    private func validProof(_ step:Step,_ p:Data) -> Bool {
        switch step {
        case .stopped: return !p.isEmpty && p.count <= 256 << 10
        case .gone,.clean: return p.count==64 && p.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
        default: return p.isEmpty
        }
    }
    func stopFence() throws -> Data {
        guard let entire=try witness(.stopped,entire:true) else { throw workError(EBUSY) }
        return Self.fingerprint(entire) // Includes original mark's physical inode/birth, projection SHA and every final row.
    }

    func marked(_ step: Step) throws -> Bool { try witness(step) != nil }

    // Stopped carries the canonical final generation set. Gone and clean each pin
    // that same physical stopped record's SHA. Prerequisite markers carry no payload.
    func witness(_ step: Step, entire:Bool = false, persist:Bool = false) throws -> Data? {
        guard !persist || step == .gone || step == .clean else { throw workError(EINVAL) }
        try check()
        var prior = stat()
        if fstatat(directory.fileDescriptor, step.rawValue, &prior, AT_SYMLINK_NOFOLLOW) != 0 {
            if errno == ENOENT { return nil }; throw workError(errno)
        }
        let expected = bytes(step, prior)
        guard (prior.st_mode & 0o177777) == 0o100400, prior.st_nlink == 1, prior.st_flags == 0,
              prior.st_uid == geteuid(),
              (step == .stopped ? (prior.st_size > expected.count && prior.st_size <= expected.count + (256 << 10))
                                : prior.st_size == expected.count + (step == .gone || step == .clean ? 64 : 0)) else { throw workError(ESTALE) }
        let fd = step.rawValue.withCString { zxs_open_marker(directory.fileDescriptor, $0, prior.st_ino,
            Int64(prior.st_birthtimespec.tv_sec), Int64(prior.st_birthtimespec.tv_nsec)) }
        guard fd >= 0 else { throw workError(errno) }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        return try withExtendedLifetime(file) {
            func same() throws {
                var s = stat(), n = stat()
                guard fstat(fd,&s) == 0, fstatat(directory.fileDescriptor, step.rawValue, &n, AT_SYMLINK_NOFOLLOW) == 0,
                      s.st_ino == prior.st_ino, s.st_dev == prior.st_dev, n.st_ino == s.st_ino, n.st_dev == s.st_dev,
                      s.st_size == prior.st_size, s.st_ctimespec.tv_sec == prior.st_ctimespec.tv_sec,
                      s.st_ctimespec.tv_nsec == prior.st_ctimespec.tv_nsec,
                      step.rawValue.withCString({ zxs_work_spelling(fd,$0) }) == 0
                else { throw workError(ESTALE) }
                try check()
            }
            try same()
            let length=Int(prior.st_size)
            if persist, flock(fd,LOCK_SH|LOCK_NB) != 0 { throw workError(errno) }
            let observed=try readBytes(fd,length)
            guard Data(observed.prefix(expected.count))==expected else { throw workError(ESTALE) }
            try same()
            if persist {
                guard fsync(fd)==0, fsync(directory.fileDescriptor)==0, fcntl(fd,F_FULLFSYNC)==0
                else { throw workError(errno) }
                guard try readBytes(fd,length)==observed else { throw workError(ESTALE) }
                try same()
                try file.close() // A consumer close/flush failure refuses positive handoff, even for a visible decision.
            }
            let proof=Data(observed.dropFirst(expected.count))
            guard validProof(step,proof) else { throw workError(ESTALE) }
            return entire ? Data(observed) : proof
        }
    }
}
