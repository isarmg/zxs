import Foundation

// One app-private working file. The exact UTF-8 bytes are the future full-file
// submission input; opening and editing never grant authority over an archive.
struct LocalDraft {
    static let byteLimit = 256 * 1024
    private static let lock = NSLock()
    private let folder: URL

    enum Failure: Error, Equatable { case tooLarge, invalidText, changed }

    struct Revision: Equatable {
        let bytes: Data
        var text: String { String(decoding: bytes, as: UTF8.self) }
    }

    init(folder: URL) { self.folder = folder }

    static func applicationStore() throws -> Self {
        let support = try FileManager.default.url(for: .applicationSupportDirectory,
            in: .userDomainMask, appropriateFor: nil, create: true)
        return Self(folder: support.appendingPathComponent("MobileDraft", isDirectory: true))
    }

    private var location: URL { folder.appendingPathComponent("text.utf8", isDirectory: false) }

    func open() throws -> Revision? {
        Self.lock.lock(); defer { Self.lock.unlock() }
        return try read()
    }

    func save(_ text: String, replacing original: Revision?) throws -> Revision {
        guard text.utf8.count <= Self.byteLimit else { throw Failure.tooLarge }
        let next = Revision(bytes: Data(text.utf8))
        Self.lock.lock(); defer { Self.lock.unlock() }
        guard try read() == original else { throw Failure.changed }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try next.bytes.write(to: location, options: [.atomic, .completeFileProtection])
        guard try read() == next else { throw Failure.changed }
        return next
    }

    private func read() throws -> Revision? {
        let input: FileHandle
        do {
            input = try FileHandle(forReadingFrom: location)
        } catch let error as NSError where error.domain == NSCocoaErrorDomain
                && error.code == NSFileReadNoSuchFileError {
            return nil
        }
        defer { try? input.close() }
        var bytes = Data()
        while bytes.count <= Self.byteLimit {
            guard let part = try input.read(upToCount: Self.byteLimit + 1 - bytes.count),
                  !part.isEmpty else { break }
            bytes.append(part)
        }
        guard bytes.count <= Self.byteLimit else { throw Failure.tooLarge }
        guard String(data: bytes, encoding: .utf8) != nil else { throw Failure.invalidText }
        return Revision(bytes: bytes)
    }
}
