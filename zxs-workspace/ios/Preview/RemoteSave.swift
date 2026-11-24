import CryptoKit
import Darwin
import Foundation

// R3 full-file SAVE, frozen independently of URL loading or UI. The destination
// is an opaque authenticated identity provided by the future connection layer.
struct RemoteSave: Sendable {
    enum Target: Codable, Equatable, Sendable {
        case firstMain(name: String)
        case append(branch: String, head: String)
        case forkMain(branch: String, from: String)

        var branch: String {
            switch self {
            case .firstMain: return "main"
            case let .append(branch, _), let .forkMain(branch, _): return branch
            }
        }

        var valid: Bool {
            switch self {
            case let .firstMain(name):
                let bytes = name.utf8
                guard (1...255).contains(bytes.count), name.utf16.count <= 255,
                      bytes.last != 46, bytes.last != 32 else { return false }
                return name.unicodeScalars.allSatisfy {
                    let value = $0.value
                    return !(value <= 31 || (127...159).contains(value)
                        || #"<>:"/\|?*"#.utf8.contains(UInt8(exactly: value) ?? 0))
                }
            case let .append(branch, head):
                return (branch == "main" || RemoteSave.validID(branch)) && RemoteSave.validID(head)
            case let .forkMain(branch, from):
                return RemoteSave.validID(branch) && RemoteSave.validID(from)
            }
        }

        var receiptJSON: String {
            switch self {
            case let .firstMain(name): return #"{"kind":"first_main","name":"\#(name)"}"#
            case let .append(branch, head):
                return #"{"kind":"branch","branch":"\#(branch)","expected_head":"\#(head)"}"#
            case let .forkMain(branch, from):
                return #"{"kind":"historical_main","branch":"\#(branch)","from_node":"\#(from)"}"#
            }
        }
    }

    enum Failure: Error { case invalidIntent, invalidStoredRecord, syncFailed(Int32) }
    enum Reply: Equatable { case committed(UInt64), absent, conflict, unknown }
    struct Request: Sendable {
        let method: String
        let path: String
        let headers: [String: String]
        let body: Data?
    }

    private struct Record: Codable, Sendable {
        let version: Int
        let destination: String
        let container: String
        let operation: String
        let node: String
        let base: UInt64
        let target: Target
        let payload: Data
        let digest: String
    }

    private struct Envelope: Codable {
        let record: Data
        let sha256: String
    }

    private struct SavedReceipt: Codable {
        let exact: Data
        let sha256: String
    }

    private let record: Record
    var destination: String { record.destination }
    var operation: String { record.operation }

    var summary: String {
        let title: String
        switch record.target {
        case let .firstMain(name): title = name
        case let .append(branch, head): title = "追加到 " + branch + " / " + head
        case let .forkMain(branch, from): title = "从 main 的 " + from + " 分叉到 " + branch
        }
        return title + "\n" + String(record.payload.count) + " B · SHA-256 " + record.digest
            + "\n操作号 " + record.operation
    }

    init(destination: String, container: String, operation: String, node: String, base: UInt64,
         target: Target, draft: LocalDraft.Revision) throws {
        guard !destination.isEmpty, destination.utf8.count <= 512,
              Self.validID(container), Self.validID(operation), Self.validID(node),
              base < UInt64.max - 1, target.valid, draft.bytes.count <= LocalDraft.byteLimit,
              String(data: draft.bytes, encoding: .utf8) != nil else { throw Failure.invalidIntent }
        switch target {
        case .firstMain: guard base == 0 else { throw Failure.invalidIntent }
        case let .append(_, head), let .forkMain(_, head):
            guard base > 0, node != head else { throw Failure.invalidIntent }
        }
        record = Record(version: 1, destination: destination, container: container, operation: operation,
                        node: node, base: base, target: target, payload: draft.bytes,
                        digest: Self.hex(SHA256.hash(data: draft.bytes)))
    }

    init(reopening bytes: Data) throws {
        guard bytes.count <= 512 * 1024 else { throw Failure.invalidStoredRecord }
        let decoder = JSONDecoder()
        let envelope = try decoder.decode(Envelope.self, from: bytes)
        guard envelope.sha256 == Self.hex(SHA256.hash(data: envelope.record)),
              try Self.encodeCanonical(envelope) == bytes else { throw Failure.invalidStoredRecord }
        let stored = try decoder.decode(Record.self, from: envelope.record)
        guard try Self.encodeCanonical(stored) == envelope.record else { throw Failure.invalidStoredRecord }
        try self.init(destination: stored.destination, container: stored.container,
                      operation: stored.operation, node: stored.node, base: stored.base, target: stored.target,
                      draft: LocalDraft.Revision(bytes: stored.payload))
        guard stored.version == 1, stored.digest == record.digest else { throw Failure.invalidStoredRecord }
    }

    func confirmation() throws -> RemoteSaveConsent {
        RemoteSaveConsent(operation: operation, reviewed: try encode())
    }

    func encode() throws -> Data {
        let bytes = try Self.encodeCanonical(record)
        return try Self.encodeCanonical(Envelope(record: bytes, sha256: Self.hex(SHA256.hash(data: bytes))))
    }

    fileprivate func encodeReceipt(_ exact: Data) throws -> Data {
        var bound = try encode()
        bound.append(0)
        bound.append(exact)
        return try Self.encodeCanonical(SavedReceipt(exact: exact, sha256: Self.hex(SHA256.hash(data: bound))))
    }

    fileprivate func decodeReceipt(_ encoded: Data) throws -> Data {
        let stored = try JSONDecoder().decode(SavedReceipt.self, from: encoded)
        guard try encodeReceipt(stored.exact) == encoded else { throw Failure.invalidStoredRecord }
        return stored.exact
    }

    private static func encodeCanonical(_ value: some Encodable) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(value)
    }

    fileprivate static func validID(_ id: String) -> Bool {
        id.utf8.count == 32 && id.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
            && id.utf8.contains { $0 != 48 }
    }
    private static func hex<S: Sequence>(_ bytes: S) -> String where S.Element == UInt8 {
        let digits = Array("0123456789abcdef".utf8)
        return String(decoding: bytes.flatMap { [digits[Int($0 >> 4)], digits[Int($0 & 15)]] }, as: UTF8.self)
    }
    private var containerPath: String { "/zxs/v1/containers/" + record.container }

    func lookup(for authenticatedDestination: String) throws -> Request {
        try checkDestination(authenticatedDestination)
        return Request(method: "GET", path: containerPath + "/operations/" + record.operation, headers: [:], body: nil)
    }

    func upload(for authenticatedDestination: String) throws -> Request {
        try checkDestination(authenticatedDestination)
        var headers = ["Content-Type": "application/octet-stream", "Content-Length": String(record.payload.count),
                       "Zxs-Operation": record.operation, "Zxs-Node": record.node, "Zxs-Sha256": record.digest]
        switch record.target {
        case let .firstMain(name):
            headers["Zxs-Base-Node"] = "0"
            headers["Zxs-Initial-Name"] = Self.hex(name.utf8)
        case let .append(_, head): headers["Zxs-Base-Node"] = head
        case let .forkMain(_, from): headers["Zxs-Fork-From-Main"] = from
        }
        return Request(method: "POST", path: containerPath + "/branches/" + record.target.branch + "/nodes",
                       headers: headers, body: record.payload)
    }

    private func checkDestination(_ authenticatedDestination: String) throws {
        guard authenticatedDestination.utf8.elementsEqual(record.destination.utf8) else {
            throw Failure.invalidIntent
        }
    }

    // R3 emits these receipts in one known UTF-8 shape. Display-name validation
    // excludes the scalars its serializer would escape. Compare that shape
    // directly so duplicate/extra keys or numeric coercion cannot confirm a save.
    func receipt(status: Int, bytes: Data, isLookup: Bool, authenticatedDestination: String) throws -> Reply {
        try checkDestination(authenticatedDestination)
        if status == 409 { return .conflict }
        if status == 404 {
            return isLookup && bytes == Data(#"{"state":"not_found"}"#.utf8) ? .absent : .unknown
        }
        guard status == 200, bytes.count <= 2048 else { return .unknown }
        let prefix = Data(#"{"container_id":"\#(record.container)","operation_id":"\#(record.operation)","sequence":"#.utf8)
        let suffix = Data(#","kind":"save_node","node_id":"\#(record.node)","length":\#(record.payload.count),"sha256":"\#(record.digest)","target":\#(record.target.receiptJSON)}"#.utf8)
        guard bytes.count > prefix.count + suffix.count,
              bytes.prefix(prefix.count).elementsEqual(prefix), bytes.suffix(suffix.count).elementsEqual(suffix) else {
            return .unknown
        }
        let number = bytes.dropFirst(prefix.count).dropLast(suffix.count)
        guard number.count <= 20, number.first != 48,
              number.allSatisfy({ (48...57).contains($0) }),
              let sequence = UInt64(String(decoding: number, as: UTF8.self)),
              record.base < sequence, sequence < UInt64.max else { return .unknown }
        return .committed(sequence)
    }
}

struct RemoteSaveConsent: Sendable {
    let operation: String
    fileprivate let reviewed: Data

    fileprivate init(operation: String, reviewed: Data) {
        self.operation = operation
        self.reviewed = reviewed
    }

    func permits(_ original: RemoteSave) throws -> Bool {
        guard original.operation == operation else { return false }
        return try original.encode() == reviewed
    }
}

// Application-owned directory only. The operation record is immutable after
// publication. A later sender must separately persist attempts before POST.
struct RemoteSaveStore {
    private static let maxRecord = 512 * 1024
    private let directory: URL

    init(directory: URL) { self.directory = directory }

    static func applicationStore() throws -> Self {
        let support = try FileManager.default.url(for: .applicationSupportDirectory,
            in: .userDomainMask, appropriateFor: nil, create: true)
        return Self(directory: support.appendingPathComponent("RemoteSave", isDirectory: true))
    }

    func operationIDs() throws -> [String] {
        guard let children = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil,
                options: [.skipsSubdirectoryDescendants], errorHandler: { _, _ in false }) else { return [] }
        var result = Set<String>()
        var scanned = 0
        for case let url as URL in children {
            scanned += 1
            guard scanned <= 4096 else { throw RemoteSave.Failure.invalidStoredRecord }
            if url.pathExtension == "json" || url.pathExtension == "attempt" {
                let id = url.deletingPathExtension().lastPathComponent
                if RemoteSave.validID(id) { result.insert(id) }
            }
        }
        return result.sorted()
    }

    func retain(_ save: RemoteSave) throws {
        try publish(save.encode(), at: location(save.operation))
    }

    func attempted(_ save: RemoteSave) throws -> Bool {
        guard let found = try loadOptional(location(save.operation, ending: ".attempt")) else { return false }
        guard found == (try save.encode()) else { throw RemoteSave.Failure.invalidStoredRecord }
        return true
    }

    func admitAttempt(_ save: RemoteSave, after attempted: Bool,
                      authenticatedDestination: String) throws -> RemoteSave.Request {
        // Caller already made one same-operation authority lookup. Re-sync
        // discovery after a crash, then persist the matching marker before
        // the caller can construct its body-bearing request.
        guard try self.attempted(save) == attempted else { throw RemoteSave.Failure.invalidStoredRecord }
        let original = try location(save.operation)
        let expected = try save.encode()
        guard try load(original) == expected else { throw RemoteSave.Failure.invalidStoredRecord }
        let handle = try FileHandle(forWritingTo: original)
        defer { try? handle.close() }
        try handle.synchronize()
        try syncDirectory(directory)
        try syncDirectory(directory.deletingLastPathComponent())
        guard try load(original) == expected else { throw RemoteSave.Failure.invalidStoredRecord }
        try publish(expected, at: location(save.operation, ending: ".attempt"))
        guard try load(original) == expected else { throw RemoteSave.Failure.invalidStoredRecord }
        return try save.upload(for: authenticatedDestination)
    }

    func cachedReceipt(_ save: RemoteSave) throws -> Data? {
        guard let stored = try loadOptional(location(save.operation, ending: ".receipt")) else { return nil }
        return try save.decodeReceipt(stored)
    }

    func recordReceipt(_ bytes: Data, for save: RemoteSave) throws {
        guard case .committed = try save.receipt(status: 200, bytes: bytes, isLookup: true,
                                                authenticatedDestination: save.destination)
        else { throw RemoteSave.Failure.invalidStoredRecord }
        try publish(save.encodeReceipt(bytes), at: location(save.operation, ending: ".receipt"))
    }

    private func publish(_ body: Data, at final: URL) throws {
        let manager = FileManager.default
        try manager.createDirectory(at: directory, withIntermediateDirectories: true)
        let staged = directory.appendingPathComponent("." + UUID().uuidString)
        try body.write(to: staged, options: [.withoutOverwriting, .completeFileProtection])
        defer { try? manager.removeItem(at: staged) }
        let handle = try FileHandle(forWritingTo: staged)
        defer { try? handle.close() }
        try handle.synchronize()
        guard try load(staged) == body else { throw RemoteSave.Failure.invalidStoredRecord }
        // An identical existing record can be re-synced after uncertainty.
        // A different record at the original operation ID is never replaced.
        do {
            try manager.linkItem(at: staged, to: final)
        } catch {
            guard (try? load(final)) == body else { throw error }
            let existing = try FileHandle(forWritingTo: final)
            defer { try? existing.close() }
            try existing.synchronize()
        }
        try syncDirectory(directory)
        try syncDirectory(directory.deletingLastPathComponent())
        guard try load(final) == body else { throw RemoteSave.Failure.invalidStoredRecord }
    }

    func reopen(operation: String) throws -> RemoteSave {
        let save = try RemoteSave(reopening: load(location(operation)))
        guard save.operation == operation else { throw RemoteSave.Failure.invalidStoredRecord }
        return save
    }

    func reopenForRecovery(operation: String) throws -> RemoteSave {
        if let attempted = try loadOptional(location(operation, ending: ".attempt")) {
            // Independently reopen the original marker. It may authorize
            // GET even when the primary intent is missing, never a POST.
            let save = try RemoteSave(reopening: attempted)
            guard save.operation == operation else { throw RemoteSave.Failure.invalidStoredRecord }
            return save
        }
        return try reopen(operation: operation)
    }

    private func location(_ operation: String, ending: String = ".json") throws -> URL {
        guard RemoteSave.validID(operation) else { throw RemoteSave.Failure.invalidIntent }
        return directory.appendingPathComponent(operation + ending)
    }

    private func syncDirectory(_ url: URL) throws {
        let (fd, openError) = url.withUnsafeFileSystemRepresentation { path -> (Int32, Int32) in
            guard let path else { return (-1, EINVAL) }
            let descriptor = Darwin.open(path, O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW)
            return (descriptor, errno)
        }
        guard fd >= 0 else { throw RemoteSave.Failure.syncFailed(openError) }
        defer { _ = Darwin.close(fd) }
        while Darwin.fsync(fd) != 0 {
            let syncError = errno
            guard syncError == EINTR else { throw RemoteSave.Failure.syncFailed(syncError) }
        }
    }

    private func load(_ url: URL) throws -> Data {
        guard let bytes = try loadOptional(url) else { throw RemoteSave.Failure.invalidStoredRecord }
        return bytes
    }

    private func loadOptional(_ url: URL) throws -> Data? {
        let (descriptor, openError) = url.withUnsafeFileSystemRepresentation { path -> (Int32, Int32) in
            guard let path else { return (-1, EINVAL) }
            let fd = Darwin.open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK)
            return (fd, errno)
        }
        guard descriptor >= 0 else {
            if openError == ENOENT { return nil }
            throw RemoteSave.Failure.invalidStoredRecord
        }
        let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        defer { try? handle.close() }
        var info = stat()
        guard fstat(descriptor, &info) == 0, (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              (0...Int64(Self.maxRecord)).contains(info.st_size) else {
            throw RemoteSave.Failure.invalidStoredRecord
        }
        var bytes = Data()
        while bytes.count <= Self.maxRecord {
            guard let next = try handle.read(upToCount: Self.maxRecord + 1 - bytes.count),
                  !next.isEmpty else { break }
            bytes.append(next)
        }
        guard bytes.count <= Self.maxRecord, Int64(bytes.count) == info.st_size else {
            throw RemoteSave.Failure.invalidStoredRecord
        }
        return bytes
    }
}
