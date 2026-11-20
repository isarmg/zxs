import Darwin
import Foundation

// The sole access grant lives only while a foreground selection or its draining read still owns it.
final class DocumentPresentation: NSObject, NSFilePresenter {
    let selected: URL
    var presentedItemURL: URL? { selected }
    let presentedItemOperationQueue: OperationQueue = {
        let q = OperationQueue(); q.maxConcurrentOperationCount = 1; return q
    }()
    private let lock = NSLock()
    private var valid = true
    private let onLoss: (DocumentPresentation) -> Void

    init(_ url: URL, onLoss: @escaping (DocumentPresentation) -> Void) throws {
        guard url.isFileURL, DirectoryIndex.safe(url.lastPathComponent),
              ["zip", "zxs"].contains(url.pathExtension.lowercased()),
              url.startAccessingSecurityScopedResource() else { throw PreviewError.unavailable }
        self.selected = url; self.onLoss = onLoss
        super.init()
        NSFileCoordinator.addFilePresenter(self)
    }
    deinit { selected.stopAccessingSecurityScopedResource() }

    func check() throws {
        lock.lock(); let ok = valid; lock.unlock()
        if !ok { throw PreviewError.unavailable }
    }
    private func invalidate() -> Bool {
        lock.lock(); let first = valid; valid = false; lock.unlock()
        return first
    }
    func retire() {
        // The invalidator owns unregistering, including when a presenter callback
        // has already notified the UI but is still relinquishing the original.
        if invalidate() { NSFileCoordinator.removeFilePresenter(self) }
    }
    private func changed() {
        guard invalidate() else { return }
        onLoss(self) // schedules UI cleanup without waiting for UI or coordination
        NSFileCoordinator.removeFilePresenter(self)
    }
    func presentedItemDidChange() { changed() }
    func presentedItemDidMove(to newURL: URL) { changed() } // never follow a replacement
    func relinquishPresentedItem(toWriter writer: @escaping ((() -> Void)?) -> Void) {
        let first = invalidate()
        if first { onLoss(self) } // revoke visible plaintext before waiting for native teardown
        writer(nil)
        if first { NSFileCoordinator.removeFilePresenter(self) }
    }
    func accommodatePresentedItemDeletion(completionHandler: @escaping (Error?) -> Void) {
        changed(); completionHandler(nil)
    }
    @available(iOS 17.4, *)
    func accommodatePresentedItemEviction(completionHandler: @escaping (Error?) -> Void) {
        changed()
        // The UI or an in-flight coordinated read can still own the FD. Never wait here or
        // claim the provider can evict it; a later request may succeed after ownership drains.
        completionHandler(NSError(domain: NSPOSIXErrorDomain, code: Int(EBUSY)))
    }
}

final class HeldArchive {
    let scope: DocumentPresentation
    private let fd: Int32, original: stat
    var size: Int { Int(original.st_size) }

    init(scope: DocumentPresentation, at spelling: UnsafePointer<CChar>) throws {
        let opened = Darwin.open(spelling, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard opened >= 0 else { throw PreviewError.unavailable }
        var s = stat()
        guard fstat(opened, &s) == 0, s.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
              s.st_ino != 0, s.st_dev != 0, s.st_nlink != 0,
              s.st_size >= 22, s.st_size <= off_t(DirectoryIndex.maxSize)
        else { _ = Darwin.close(opened); throw PreviewError.unavailable }
        self.scope = scope; fd = opened; original = s
    }
    deinit { _ = Darwin.close(fd) }

    private func same(_ b: stat) -> Bool {
        let a = original
        return a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_mode == b.st_mode &&
            a.st_nlink == b.st_nlink && a.st_size == b.st_size &&
            a.st_mtimespec.tv_sec == b.st_mtimespec.tv_sec && a.st_mtimespec.tv_nsec == b.st_mtimespec.tv_nsec &&
            a.st_ctimespec.tv_sec == b.st_ctimespec.tv_sec && a.st_ctimespec.tv_nsec == b.st_ctimespec.tv_nsec
    }
    // Presentation never coordinates a new access, follows a name, or reads member bytes.
    func checkPreview() throws {
        try scope.check()
        var held = stat()
        guard fstat(fd, &held) == 0, same(held) else { throw PreviewError.unavailable }
        try scope.check()
    }

    func withReads<T>(at name: UnsafePointer<CChar>, check: () throws -> Void,
                      _ body: (_ fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> T) throws -> T {
        func validate() throws {
            try check(); try scope.check()
            var held = stat(), named = stat()
            guard fstat(fd, &held) == 0, lstat(name, &named) == 0, same(held), same(named)
            else { throw PreviewError.unavailable }
        }
        try validate()
        var untilCheck = 8 << 20
        let value = try body { off, buffer in
            try check(); try self.scope.check()
            try requirePreview(off >= 0 && off <= self.size - buffer.count)
            var got = 0
            while got < buffer.count {
                try check(); try self.scope.check()
                if untilCheck <= 0 { try validate(); untilCheck = 8 << 20 }
                let count = min(65_536, buffer.count - got)
                let n = Darwin.pread(self.fd, buffer.baseAddress!.advanced(by: got), count, off_t(off + got))
                if n < 0 && errno == EINTR { continue }
                guard n > 0 && n <= count else { throw PreviewError.unavailable }
                got += n; untilCheck -= n
            }
        }
        try validate()
        return value
    }
}

struct Report {
    let archive: HeldArchive, directory: DirectoryIndex
    var name: String { archive.scope.selected.lastPathComponent }
}

final class DocumentRead {
    private let lock = NSLock()
    private var cancelled = false
    private var coordinating: NSFileCoordinator?

    func cancel() {
        lock.lock(); cancelled = true; let c = coordinating; lock.unlock()
        c?.cancel()
    }
    func check() throws {
        lock.lock(); let stop = cancelled; lock.unlock()
        if stop { throw PreviewError.cancelled }
    }
    func present(_ selected: URL, onLoss: @escaping (DocumentPresentation) -> Void) throws -> DocumentPresentation {
        try check()
        return try DocumentPresentation(selected, onLoss: onLoss)
    }

    private func coordinated<T>(_ scope: DocumentPresentation,
                                _ body: (UnsafePointer<CChar>) throws -> T) throws -> T {
        try check(); try scope.check()
        let c = NSFileCoordinator(filePresenter: scope)
        lock.lock()
        if cancelled { lock.unlock(); throw PreviewError.cancelled }
        coordinating = c; lock.unlock()
        defer { lock.lock(); coordinating = nil; lock.unlock() }
        var coordinationError: NSError?, result: Result<T, Error>?
        c.coordinate(readingItemAt: scope.selected, options: .withoutChanges, error: &coordinationError) { located in
            result = Result {
                try self.check(); try scope.check()
                guard located.isFileURL,
                      located.lastPathComponent.utf8.elementsEqual(scope.selected.lastPathComponent.utf8)
                else { throw PreviewError.unavailable }
                return try located.withUnsafeFileSystemRepresentation { name in
                    guard let name = name else { throw PreviewError.unavailable }
                    return try body(name) // only the accessor URL, never a derived sibling or saved path
                }
            }
        }
        try check(); try scope.check()
        guard coordinationError == nil, let result = result else { throw PreviewError.unavailable }
        return try result.get()
    }

    func run(_ scope: DocumentPresentation) throws -> Report {
        try coordinated(scope) { name in
            let archive = try HeldArchive(scope: scope, at: name)
            let index = try archive.withReads(at: name, check: self.check) { fill in
                try DirectoryIndex.read(size: archive.size, check: { try self.check(); try scope.check() }) { off, n in
                    var bytes = [UInt8](repeating: 0, count: n)
                    try bytes.withUnsafeMutableBytes { (buf: UnsafeMutableRawBufferPointer) in try fill(off, buf) }
                    return bytes
                }
            }
            return Report(archive: archive, directory: index)
        }
    }
    func verify(_ report: Report, file: Member, mode: MemberReadMode) throws -> MemberCRC {
        let archive = report.archive // hold original FD and scope through the whole accessor, including invalidation
        return try coordinated(archive.scope) { name in
            try archive.withReads(at: name, check: self.check) { fill in
                try report.directory.checkMember(file, mode: mode,
                    check: { try self.check(); try archive.scope.check() }, fill: fill)
            }
        }
    }

    func page(_ report: Report, file: Member, proof: DirectoryIndex.TextIndex, index: Int) throws -> String {
        let archive = report.archive
        return try coordinated(archive.scope) { name in
            try archive.withReads(at: name, check: self.check) { fill in
                try report.directory.textPage(file, proof: proof, page: index,
                    check: { try self.check(); try archive.scope.check() }, fill: fill)
            }
        }
    }
}
