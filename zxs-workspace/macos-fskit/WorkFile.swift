import CryptoKit
import Darwin
import Foundation
import FSKit

// These values must come from the controller's existing durable, private work
// registration, not from URL input or statting a new selection under a display name.
struct WorkWitness {
    let volumeHex: String
    let inode: UInt64
    let birthSec: Int64
    let birthNsec: Int64

    func check(_ fd: Int32, directory: Bool) throws {
        let code = volumeHex.withCString {
            zxs_work_evidence(fd, directory ? 1 : 0, $0, inode, birthSec, birthNsec)
        }
        if code != 0 { throw workError(code) }
    }
}

func workError(_ value: Int32) -> NSError {
    NSError(domain: NSPOSIXErrorDomain, code: Int(value))
}

struct WorkGeneration {
    let size: Int64
    let mtimeSec: Int64, mtimeNsec: Int64
    let ctimeSec: Int64, ctimeNsec: Int64
    let sha256: String

    func matches(_ s: stat) -> Bool {
        size == s.st_size && mtimeSec == Int64(s.st_mtimespec.tv_sec) && mtimeNsec == Int64(s.st_mtimespec.tv_nsec)
            && ctimeSec == Int64(s.st_ctimespec.tv_sec) && ctimeNsec == Int64(s.st_ctimespec.tv_nsec)
    }
}

private final class OwnedFD {
    private(set) var number: Int32
    init(_ fd: Int32) { number = fd }
    func closeOnce() throws {
        let fd = number
        number = -1
        if fd >= 0 && Darwin.close(fd) != 0 { throw workError(errno) }
    }
    deinit { if number >= 0 { _ = Darwin.close(number) } }
}

@available(macOS 27.0, *)
final class WorkFile {
    static let maxIO = 16 << 20
    static let maxFile: off_t = 128 << 30
    private let lock = NSLock()
    private let parent: OwnedFD
    private let parentID: WorkWitness
    private let fileID: WorkWitness
    private let leaf: String
    private let reader: OwnedFD
    private let writer: OwnedFD
    private let uid = getuid(), gid = getgid()
    private var last: stat
    private var failure: Error?
    private var originalOpen: FSVolume.OpenModes = []
    private var stopping = false
    private var writerClosed = false
    private var virtualID: FSItem.Identifier = .invalid
    private static let registrationLock = NSLock()

    // The registry loader must have independently authorized a one-use, private
    // RESOURCE for this volume. This primitive accepts a held directory, not a path.
    init(parent: FileHandle, parentID: WorkWitness, fileID: WorkWitness, generation: WorkGeneration,
         container: String) throws {
        guard container.utf8.count == 32, !container.utf8.allSatisfy({ $0 == 48 }),
              container.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
              parentID.volumeHex == fileID.volumeHex else { throw workError(EINVAL) }
        let name = "body-" + container
        try parentID.check(parent.fileDescriptor, directory: true)
        let duplicated = fcntl(parent.fileDescriptor, F_DUPFD_CLOEXEC, 0)
        guard duplicated >= 0 else { throw workError(errno) }
        let directory = OwnedFD(duplicated)
        try parentID.check(directory.number, directory: true)
        func acquire(_ flags: Int32) throws -> OwnedFD {
            let fd = name.withCString {
                zxs_open_work_leaf(directory.number, $0, flags, fileID.inode, fileID.birthSec, fileID.birthNsec)
            }
            guard fd >= 0 else { throw workError(errno) }
            let owner = OwnedFD(fd)
            try fileID.check(fd, directory: false)
            let actual = fcntl(fd, F_GETFL)
            guard actual >= 0, actual & O_ACCMODE == flags, actual & O_APPEND == 0 else { throw workError(EIO) }
            return owner
        }
        let ro = try acquire(O_RDONLY), rw = try acquire(O_RDWR)
        guard flock(rw.number, LOCK_EX | LOCK_NB) == 0 else { throw workError(errno) }
        self.parent = directory; self.parentID = parentID; self.fileID = fileID
        self.leaf = name; self.reader = ro; self.writer = rw; self.last = stat()
        self.last = try observed()
        guard generation.matches(last), generation.sha256.utf8.count == 64 else { throw workError(ESTALE) }
        var hash = SHA256(), at: off_t = 0
        var block = [UInt8](repeating: 0, count: 1 << 20)
        while at < generation.size {
            let amount = Int(min(off_t(block.count), generation.size - at))
            let n = block.withUnsafeMutableBytes { pread(ro.number, $0.baseAddress!, amount, at) }
            if n < 0 && errno == EINTR { continue }
            guard n > 0 else { throw workError(n == 0 ? EIO : errno) }
            hash.update(data: Data(block.prefix(n)))
            at += off_t(n)
        }
        guard hash.finalize().map({ String(format: "%02x", $0) }).joined() == generation.sha256,
              generation.matches(try current()) else { throw workError(ESTALE) }
    }

    private func withLock<T>(_ work: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try work()
    }

    private func observed() throws -> stat {
        try parentID.check(parent.number, directory: true)
        try fileID.check(reader.number, directory: false)
        let spelled = leaf.withCString { zxs_work_spelling(reader.number, $0) }
        guard spelled == 0 else { throw workError(spelled) }
        if !writerClosed { try fileID.check(writer.number, directory: false) }
        var named = stat(), held = stat()
        let rc = leaf.withCString { fstatat(parent.number, $0, &named, AT_SYMLINK_NOFOLLOW) }
        guard rc == 0, fstat(reader.number, &held) == 0 else { throw workError(errno) }
        guard named.st_ino == held.st_ino, named.st_dev == held.st_dev, held.st_size >= 0,
              held.st_size <= Self.maxFile else { throw workError(ESTALE) }
        return held
    }

    private func current() throws -> stat {
        if let failure { throw failure }
        do {
            let s = try observed()
            guard s.st_size == last.st_size,
                  s.st_mtimespec.tv_sec == last.st_mtimespec.tv_sec,
                  s.st_mtimespec.tv_nsec == last.st_mtimespec.tv_nsec,
                  s.st_ctimespec.tv_sec == last.st_ctimespec.tv_sec,
                  s.st_ctimespec.tv_nsec == last.st_ctimespec.tv_nsec else { throw workError(ESTALE) }
            return s
        } catch { failure = error; throw error }
    }

    static let provided: FSItem.Attribute = [
        .fileID, .parentID, .type, .mode, .linkCount, .uid, .gid, .size, .allocSize, .flags,
        .accessTime, .modifyTime, .changeTime, .birthTime, .supportsLimitedXAttrs, .inhibitKernelOffloadedIO,
    ]

    private func requireAttributes(_ request: FSItem.GetAttributesRequest) throws {
        guard request.wantedAttributes.subtracting(Self.provided).isEmpty else { throw workError(ENOTSUP) }
    }

    private func attributes(_ s: stat) -> FSItem.Attributes {
        let a = FSItem.Attributes() // Allocate the FSKit sequence INSIDE the same data-generation lock.
        a.fileID = virtualID; a.parentID = .rootDirectory; a.type = .file
        a.uid = uid; a.gid = gid; a.mode = 0o600; a.linkCount = 1
        a.size = UInt64(s.st_size); a.allocSize = UInt64(max(0, s.st_blocks)) * 512
        a.modifyTime = s.st_mtimespec; a.changeTime = s.st_ctimespec; a.birthTime = s.st_birthtimespec
        a.flags = 0; a.accessTime = s.st_atimespec; a.supportsLimitedXAttrs = false
        a.inhibitKernelOffloadedIO = true
        return a
    }

    func matchesRegistration(container: String, volume: String) -> Bool {
        leaf == "body-" + container && fileID.volumeHex == volume
    }

    // All-or-nothing ownership. A second volume cannot partially capture an earlier
    // registration and cannot poison another volume's still-live data callbacks.
    static func bind(_ proposed: [(WorkFile, FSItem.Identifier)]) throws {
        registrationLock.lock()
        defer { registrationLock.unlock() }
        var seen = Set<ObjectIdentifier>()
        for (file, id) in proposed {
            guard id != .invalid, seen.insert(ObjectIdentifier(file)).inserted else { throw workError(EINVAL) }
            try file.withLock {
                guard file.virtualID == .invalid else { throw workError(EBUSY) }
                _ = try file.current()
            }
        }
        for (file, id) in proposed { file.withLock { file.virtualID = id } }
    }

    static func authorize(_ context: FSContext) throws {
        let uid = getuid(), gid = getgid()
        guard uid != 0, uid == geteuid(), gid == getegid(),
              context.realUserID == Int(uid), context.effectiveUserID == Int(uid),
              context.realGroupID == Int(gid), context.effectiveGroupID == Int(gid)
        else { throw workError(EACCES) }
    }

    func opened(_ modes: FSVolume.OpenModes) throws {
        try withLock {
            guard !stopping, !writerClosed else { throw workError(EBUSY) }
            guard !modes.isEmpty, modes.subtracting([.read, .write]).isEmpty else { throw workError(EACCES) }
            if modes.contains(.write), originalOpen.contains(.write) { throw workError(EBUSY) }
            _ = try current()
            originalOpen.formUnion(modes) // FSKit tracks an item, not a distinct userspace fd per open.
        }
    }

    func cacheUpgrade() throws {
        try withLock {
            guard !stopping, !originalOpen.isEmpty else { throw workError(EBUSY) }
            _ = try current()
        }
    }

    func closed(_ error: Error? = nil) {
        withLock {
            if let error { failure = failure ?? error; return }
            guard !originalOpen.isEmpty else { failure = failure ?? workError(EBADF); return }
            originalOpen = [] // Last item reference only. Never submit, seal, or suggest the volume is gone.
        }
    }

    func fileAttributes(_ request: FSItem.GetAttributesRequest) throws -> FSItem.Attributes {
        try withLock {
            try requireAttributes(request)
            return attributes(try current())
        }
    }

    func read(at: off_t, count: Int, buffer: FSMutableFileDataBuffer) throws -> FSReadFileResult {
        try withLock {
            guard originalOpen.contains(.read) else { throw workError(EBADF) }
            guard at >= 0, count >= 0, count <= Self.maxIO, count <= buffer.length else { throw workError(EINVAL) }
            try requireAttributes(FSReadFileResult.requestedAttributes)
            let s = try current()
            let wanted = Int(min(off_t(count), max(0, s.st_size - at)))
            if wanted > 0 {
                try buffer.withUnsafeMutableBytes { mem in
                    guard let start = mem.baseAddress, mem.count >= wanted else { throw workError(EINVAL) }
                    var got = 0
                    while got < wanted {
                        let n = pread(reader.number, start.advanced(by: got), wanted - got, at + off_t(got))
                        if n < 0 && errno == EINTR { continue }
                        guard n > 0 else { throw workError(n == 0 ? EIO : errno) }
                        got += n
                    }
                }
            }
            let after = try current()
            guard let result = FSReadFileResult(bytesRead: wanted, itemAttributes: attributes(after)) else {
                throw workError(EIO)
            }
            return result // Never pad to request length; even an already-open reader observes current EOF.
        }
    }

    private func changed<T>(_ change: () throws -> Void, _ respond: (FSItem.Attributes) throws -> T) throws -> T {
        _ = try current()
        do {
            try change()
            last = try observed()
            return try respond(attributes(last))
        } catch { failure = error; throw error } // May include a partial syscall or a failed FSKit result.
    }

    func write(_ data: Data, at: off_t) throws -> FSWriteFileResult {
        try withLock {
            guard originalOpen.contains(.write), !writerClosed else { throw workError(EBADF) }
            guard at >= 0, data.count <= Self.maxIO, at <= Self.maxFile - off_t(data.count)
            else { throw workError(EINVAL) }
            try requireAttributes(FSWriteFileResult.requestedAttributes)
            return try changed({
                try data.withUnsafeBytes { mem in
                    var n = 0
                    while n < mem.count {
                        guard let start = mem.baseAddress else { throw workError(EIO) }
                        let wrote = pwrite(writer.number, start.advanced(by: n), mem.count - n, at + off_t(n))
                        if wrote < 0 && errno == EINTR { continue }
                        guard wrote > 0 else { throw workError(wrote == 0 ? EIO : errno) }
                        n += wrote
                    }
                }
            }, { a in
                guard let r = FSWriteFileResult(bytesWritten: data.count, itemAttributes: a, freeSpace: nil)
                else { throw workError(EIO) }
                return r
            })
        }
    }

    func resize(_ length: UInt64) throws -> FSSetAttributesResult {
        try withLock {
            guard !stopping, !writerClosed else { throw workError(EBUSY) }
            guard length <= UInt64(Self.maxFile) else { throw workError(EFBIG) }
            try requireAttributes(FSSetAttributesResult.requestedAttributes)
            return try changed({
                guard ftruncate(writer.number, off_t(length)) == 0 else { throw workError(errno) }
            }, { a in
                guard let r = FSSetAttributesResult(attributes: a, freeSpace: nil) else { throw workError(EIO) }
                return r
            })
        }
    }

    private func barrier() throws {
        _ = try current()
        guard !writerClosed else { throw workError(EBADF) }
        do {
            guard fsync(writer.number) == 0, fcntl(writer.number, F_FULLFSYNC) == 0
            else { throw workError(errno) }
            _ = try current()
        } catch { failure = error; throw error }
    }

    func synchronize() throws { try withLock { try barrier() } }
    func stopNewOpens() { withLock { stopping = true } }

    // Local half only. A host must also prove the ORIGINAL normal unmount/kernel drain
    // separately before borrowing the private whole file. This method cannot authorize Submit.
    func requireDrained() throws {
        try withLock {
            guard stopping, originalOpen.isEmpty, !writerClosed else { throw workError(EBUSY) }
            _ = try current()
        }
    }

    func closeLocalWriterAfterDrain() throws {
        try withLock {
            guard stopping, originalOpen.isEmpty, !writerClosed else { throw workError(EBUSY) }
            try barrier()
            writerClosed = true // Never retry close on a potentially recycled descriptor.
            do { try writer.closeOnce(); _ = try current() }
            catch { failure = error; throw error }
        }
    }
}
