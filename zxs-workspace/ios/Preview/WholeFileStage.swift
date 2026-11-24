import CryptoKit
import Darwin
import Foundation

struct WholeFileStage {
    enum Failure: Error { case invalidFile, changed, sync(Int32), retained(Identity) }

    struct Identity: Codable, Equatable, Sendable {
        let id: UUID
        let length: Int
        let sha256: String
    }

    final class Prepared {
        let identity: Identity
        fileprivate let directory: URL
        fileprivate let temporary: URL
        fileprivate let handle: FileHandle
        fileprivate var linked = false
        fileprivate var temporaryExists = true

        fileprivate init(identity: Identity, directory: URL, temporary: URL, handle: FileHandle) {
            self.identity = identity; self.directory = directory; self.temporary = temporary; self.handle = handle
        }

        deinit {
            try? handle.close()
            if temporaryExists { try? FileManager.default.removeItem(at: temporary) }
        }
    }

    private let directory: URL
    init(directory: URL) { self.directory = directory }

    static func applicationStore() throws -> Self {
        let base = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                               appropriateFor: nil, create: true)
        return Self(directory: base.appendingPathComponent("WholeFiles", isDirectory: true))
    }

    // Fill reads only the confirmed held source; validate checks its name,
    // attributes, authority, and EOF before publication.
    func freeze(id: UUID, length: Int, check: () throws -> Void,
                fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void,
                validate: () throws -> Void) throws -> Identity {
        let staged = try prepare(id: id, length: length, check: check, fill: fill, validate: validate)
        return try publish(staged, check: check)
    }

    func prepare(id: UUID, length: Int, check: () throws -> Void,
                 fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void,
                 validate: () throws -> Void) throws -> Prepared {
        guard length >= 0 else { throw Failure.invalidFile }
        try check()
        let manager = FileManager.default
        try manager.createDirectory(at: directory, withIntermediateDirectories: true,
                                    attributes: [.posixPermissions: 0o700])
        let temp = directory.appendingPathComponent("." + UUID().uuidString)
        let (fd, _) = openFile(temp, flags: O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW)
        guard fd >= 0 else { throw Failure.invalidFile }
        let output = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        var handedOff = false
        defer {
            if !handedOff {
                try? output.close()
                try? manager.removeItem(at: temp)
            }
        }

        try manager.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: temp.path)
        var digest = SHA256()
        var buffer = [UInt8](repeating: 0, count: 65_536)
        var offset = 0
        while offset < length {
            try check()
            let count = min(buffer.count, length - offset)
            try buffer.withUnsafeMutableBytes { raw in
                let slice = UnsafeMutableRawBufferPointer(rebasing: raw[..<count])
                try fill(offset, slice)
                digest.update(bufferPointer: UnsafeRawBufferPointer(slice))
                try output.write(contentsOf: Data(slice))
            }
            offset += count
        }
        try validate()
        let sha = Self.hex(digest.finalize())
        try output.seek(toOffset: 0)
        try Self.verify(output, length: length, sha256: sha, check: check)
        guard Darwin.fchmod(fd, 0o400) == 0 else { throw Failure.invalidFile }
        try output.synchronize()
        try check()
        try validate()

        try sameName(temp, fd: fd, length: length)
        let prepared = Prepared(identity: Identity(id: id, length: length, sha256: sha),
                                directory: directory, temporary: temp, handle: output)
        handedOff = true
        return prepared
    }

    func publish(_ prepared: Prepared, check: () throws -> Void) throws -> Identity {
        guard prepared.directory == directory, !prepared.linked, prepared.temporaryExists
        else { throw Failure.invalidFile }
        let identity = prepared.identity
        let fd = prepared.handle.fileDescriptor
        let final = location(identity.id)
        try sameName(prepared.temporary, fd: fd, length: identity.length)
        try check()
        let manager = FileManager.default
        // Only publication happens after every coordinated source check.
        // Do not report a later failure as cancellation or a bare absent file.
        try manager.linkItem(at: prepared.temporary, to: final)
        prepared.linked = true
        do {
            try manager.removeItem(at: prepared.temporary)
            prepared.temporaryExists = false
            try syncDirectory(directory)
            try syncDirectory(directory.deletingLastPathComponent())
            try sameName(final, fd: fd, length: identity.length)
        } catch {
            throw Failure.retained(identity)
        }
        return identity
    }

    func reopen(_ expected: Identity, check: () throws -> Void) throws -> FileHandle {
        guard expected.length >= 0 else { throw Failure.invalidFile }
        let final = location(expected.id)
        let (fd, _) = openFile(final, flags: O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK)
        guard fd >= 0 else { throw Failure.invalidFile }
        let input = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        try sameName(final, fd: fd, length: expected.length)
        try Self.verify(input, length: expected.length, sha256: expected.sha256, check: check)
        try sameName(final, fd: fd, length: expected.length)
        try input.seek(toOffset: 0)
        return input
    }

    private func location(_ id: UUID) -> URL { directory.appendingPathComponent(id.uuidString) }

    private static func verify(_ file: FileHandle, length: Int, sha256: String,
                               check: () throws -> Void) throws {
        var digest = SHA256()
        var seen = 0
        while seen < length {
            try check()
            guard let bytes = try file.read(upToCount: min(65_536, length - seen)), !bytes.isEmpty
            else { throw Failure.changed }
            seen += bytes.count
            digest.update(data: bytes)
        }
        try check()
        guard (try file.read(upToCount: 1))?.isEmpty != false,
              Self.hex(digest.finalize()) == sha256 else { throw Failure.changed }
    }

    private static func hex(_ bytes: SHA256.Digest) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    private func openFile(_ url: URL, flags: Int32) -> (Int32, Int32) {
        url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return (-1, EINVAL) }
            let fd = flags & O_CREAT == 0 ? Darwin.open(path, flags) : Darwin.open(path, flags, 0o600)
            return (fd, errno)
        }
    }

    private func syncDirectory(_ url: URL) throws {
        let (fd, code) = openFile(url, flags: O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)
        guard fd >= 0 else { throw Failure.sync(code) }
        defer { _ = Darwin.close(fd) }
        while Darwin.fsync(fd) != 0 {
            let code = errno
            guard code == EINTR else { throw Failure.sync(code) }
        }
    }

    private func sameName(_ url: URL, fd: Int32, length: Int) throws {
        var held = stat(), named = stat()
        guard fstat(fd, &held) == 0, url.withUnsafeFileSystemRepresentation({ path in
            guard let path else { return false }
            return lstat(path, &named) == 0
        }), (held.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG), held.st_nlink == 1,
              held.st_mode & 0o777 == 0o400, held.st_size == off_t(length),
              held.st_dev == named.st_dev, held.st_ino == named.st_ino
        else { throw Failure.changed }
        let observed = try FileManager.default.attributesOfItem(atPath: url.path)[.protectionKey]
        guard (observed as? FileProtectionType) == .complete
                || (observed as? String) == FileProtectionType.complete.rawValue
        else { throw Failure.changed }
    }
}
