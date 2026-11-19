import Darwin
import Foundation

struct Report {
    let name: String
    let directory: DirectoryIndex // an observation; deliberately no URL, grant or file handle
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

    func run(_ selected: URL) throws -> Report {
        try check()
        let name = selected.lastPathComponent
        guard selected.isFileURL, DirectoryIndex.safe(name),
              ["zip", "zxs"].contains(selected.pathExtension.lowercased()),
              selected.startAccessingSecurityScopedResource()
        else { throw PreviewError.unavailable }
        defer { selected.stopAccessingSecurityScopedResource() }
        let c = NSFileCoordinator(filePresenter: nil)
        lock.lock()
        if cancelled { lock.unlock(); throw PreviewError.cancelled }
        coordinating = c; lock.unlock()
        defer { lock.lock(); coordinating = nil; lock.unlock() }
        var coordinationError: NSError?
        var report: Result<Report, Error>?
        c.coordinate(readingItemAt: selected, options: .withoutChanges, error: &coordinationError) { located in
            report = Result {
                try self.check()
                guard located.isFileURL, located.lastPathComponent.utf8.elementsEqual(name.utf8)
                else { throw PreviewError.unavailable }
                // Only the URL supplied by the accessor. Neither open the original URL separately
                // nor derive a sibling path from it. Everything finishes inside this coordination.
                return try located.withUnsafeFileSystemRepresentation { spelling in
                    guard let spelling = spelling else { throw PreviewError.unavailable }
                    let fd = Darwin.open(spelling, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
                    guard fd >= 0 else { throw PreviewError.unavailable }
                    defer { _ = Darwin.close(fd) }
                    var before = stat()
                    guard fstat(fd, &before) == 0, before.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
                          before.st_ino != 0, before.st_dev != 0, before.st_nlink != 0,
                          before.st_size >= 22, before.st_size <= off_t(DirectoryIndex.maxSize)
                    else { throw PreviewError.unavailable }
                    func stillSame() throws {
                        var held = stat(), named = stat()
                        guard fstat(fd, &held) == 0, lstat(spelling, &named) == 0,
                              Self.same(before, held), Self.same(before, named)
                        else { throw PreviewError.unavailable }
                    }
                    try stillSame()
                    let index = try DirectoryIndex.read(size: Int(before.st_size), check: self.check) { off, n in
                        var result = [UInt8](repeating: 0, count: n)
                        try result.withUnsafeMutableBytes { (b: UnsafeMutableRawBufferPointer) in
                            var got = 0
                            while got < n {
                                try self.check()
                                let len = min(65536, n - got)
                                let k = Darwin.pread(fd, b.baseAddress!.advanced(by: got), len, off_t(off + got))
                                if k < 0 && errno == EINTR { continue }
                                guard k > 0 && k <= len else { throw PreviewError.unavailable }
                                got += k
                            }
                        }
                        return result
                    }
                    try self.check(); try stillSame()
                    return Report(name: name, directory: index)
                }
            }
        }
        try check()
        guard coordinationError == nil, let report = report else { throw PreviewError.unavailable }
        return try report.get()
    }

    private static func same(_ a: stat, _ b: stat) -> Bool {
        a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_mode == b.st_mode &&
        a.st_nlink == b.st_nlink && a.st_size == b.st_size &&
        a.st_mtimespec.tv_sec == b.st_mtimespec.tv_sec && a.st_mtimespec.tv_nsec == b.st_mtimespec.tv_nsec &&
        a.st_ctimespec.tv_sec == b.st_ctimespec.tv_sec && a.st_ctimespec.tv_nsec == b.st_ctimespec.tv_nsec
    }
}
