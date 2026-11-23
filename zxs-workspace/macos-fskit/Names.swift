import Darwin
import Foundation
import FSKit

/// Local projection only. A remote R3 byte spelling is never used as a submission rename.
@available(macOS 27.0, *)
struct SelectedWorkItem {
    let containerID: String
    let remoteName: Data
    let file: WorkFile
}

@available(macOS 27.0, *)
final class NameIndex {
    final class Row {
        let source: SelectedWorkItem, name: FSFileName, item = FSItem(), marker: FileHandle
        let inode: ino_t, id: FSItem.Identifier
        init(source: SelectedWorkItem, spelling: Data, fd: FileHandle, inode: ino_t, id: UInt64) {
            self.source = source; self.name = FSFileName(data: spelling); self.marker = fd
            self.inode = inode; self.id = FSItem.Identifier(id)
        }
    }

    private let parent: FileHandle, original: WorkWitness
    private var frozen: stat?
    private var byMarker: [ino_t: Row] = [:], byItem: [ObjectIdentifier: Row] = [:]
    private(set) var ordered: [Row] = []
    let caseFormat: FSVolume.CaseFormat
    let verifier = UInt64.random(in: 1...(UInt64.max >> 1)) << 1

    // The application issues a new empty 0700 index on the original work volume.
    // Decline any preexisting namespace; a failed build is not an instruction to erase or reuse it.
    init(directory: FileHandle, registered: WorkWitness, selected: [SelectedWorkItem]) throws {
        guard !selected.isEmpty, selected.count <= 128 else { throw workError(EINVAL) }
        let fd = fcntl(directory.fileDescriptor, F_DUPFD_CLOEXEC, 0)
        guard fd >= 0 else { throw workError(errno) }
        parent = FileHandle(fileDescriptor: fd, closeOnDealloc: true); original = registered
        try registered.check(fd, directory: true)
        var sensitive: Int32 = -1
        let prepared = zxs_new_names(fd, &sensitive)
        guard prepared == 0 else { throw workError(prepared) }
        caseFormat = sensitive == 1 ? .sensitive : .insensitiveCasePreserving
        var ids = Set<String>(), objects = Set<ObjectIdentifier>()
        let sorted = selected.sorted {
            if $0.remoteName == $1.remoteName { return $0.containerID < $1.containerID }
            return $0.remoteName.lexicographicallyPrecedes($1.remoteName) // Raw R3 UTF-8, never Unicode folding.
        }
        for s in sorted {
            guard s.file.matchesRegistration(container: s.containerID, volume: registered.volumeHex),
                  ids.insert(s.containerID).inserted, objects.insert(ObjectIdentifier(s.file)).inserted,
                  let decoded = String(data: s.remoteName, encoding: .utf8), Data(decoded.utf8) == s.remoteName,
                  Self.component(s.remoteName) else { throw workError(EINVAL) }
        }
        // Genuine occupied aliases go second. An early synthetic suffix must not take another
        // container's literal name before that original has had the opportunity to register.
        var collisions: [SelectedWorkItem] = []
        for selection in sorted {
            if try !insert(selection, proposed: selection.remoteName) { collisions.append(selection) }
        }
        for selection in collisions {
            var placed = false
            for sequence in 0...sorted.count {
                let proposed = Self.alias(selection, sequence: sequence)
                if try insert(selection, proposed: proposed) { placed = true; break }
            }
            guard placed else { throw workError(EEXIST) }
        }
        ordered.sort { $0.name.data.lexicographicallyPrecedes($1.name.data) }
        frozen = try rootStat()
        for r in ordered { guard try resolve(r.name.data) === r else { throw workError(ESTALE) } }
        try check()
    }

    static func component(_ bytes: Data) -> Bool {
        !bytes.isEmpty && bytes.count <= 255 && bytes != Data(".".utf8) && bytes != Data("..".utf8)
            && !bytes.contains(0) && !bytes.contains(47)
    }

    private static func alias(_ s: SelectedWorkItem, sequence: Int) -> Data {
        let name = String(decoding: s.remoteName, as: UTF8.self)
        let dot = name.lastIndex(of: ".")
        let ext = dot.flatMap { $0 != name.startIndex && name[$0...].utf8.count <= 48 ? String(name[$0...]) : nil } ?? ""
        let stem = !ext.isEmpty && dot != nil ? String(name[..<dot!]) : name
        let suffix = "-" + s.containerID + (sequence == 0 ? "" : "-\(sequence)") + ext
        // Truncate only a local alias. Never split UTF-8 or change remote metadata.
        let limit = 255 - suffix.utf8.count
        var prefix = "", n = 0
        for scalar in stem.unicodeScalars {
            let text = String(scalar)
            if n + text.utf8.count > limit { break }
            prefix += text; n += text.utf8.count
        }
        return Data((prefix + suffix).utf8)
    }

    private func insert(_ s: SelectedWorkItem, proposed: Data) throws -> Bool {
        try original.check(parent.fileDescriptor, directory: true)
        guard Self.component(proposed) else { throw workError(EINVAL) }
        let text = String(decoding: proposed, as: UTF8.self)
        let fd = text.withCString {
            openat(parent.fileDescriptor, $0, O_CREAT|O_EXCL|O_RDONLY|O_CLOEXEC|O_NONBLOCK|O_NOFOLLOW, 0o400)
        }
        if fd < 0 {
            let code = errno
            if code == EEXIST {
                // An external occupant, link or unknown generation is not a legitimate Workspace alias.
                _ = try resolve(proposed)
                return false
            }
            throw workError(code)
        }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        var st = stat(), bytes = [UInt8](repeating: 0, count: 256), n = 0
        let result = bytes.withUnsafeMutableBufferPointer { zxs_marker_name(fd, $0.baseAddress, $0.count, &n) }
        guard result == 0 else { throw workError(result) }
        let spelling = Data(bytes.prefix(n))
        guard n > 0, spelling == proposed, Self.component(spelling), String(data: spelling, encoding: .utf8) != nil,
              fstat(fd, &st) == 0, st.st_ino != 0, byMarker[st.st_ino] == nil else { throw workError(ESTALE) }
        let row = Row(source: s, spelling: spelling, fd: file, inode: st.st_ino, id: UInt64(16 + ordered.count))
        byMarker[st.st_ino] = row; byItem[ObjectIdentifier(row.item)] = row; ordered.append(row)
        guard try resolve(proposed) === row else { throw workError(ESTALE) }
        return true
    }

    func check() throws {
        try original.check(parent.fileDescriptor, directory: true)
        if let frozen {
            var now = stat()
            guard fstat(parent.fileDescriptor, &now) == 0 else { throw workError(errno) }
            guard frozen.st_mtimespec.tv_sec == now.st_mtimespec.tv_sec,
                  frozen.st_mtimespec.tv_nsec == now.st_mtimespec.tv_nsec,
                  frozen.st_ctimespec.tv_sec == now.st_ctimespec.tv_sec,
                  frozen.st_ctimespec.tv_nsec == now.st_ctimespec.tv_nsec
            else { throw workError(ESTALE) }
        }
    }

    func resolve(_ raw: Data) throws -> Row {
        try check()
        guard Self.component(raw), let text = String(data: raw, encoding: .utf8), Data(text.utf8) == raw
        else { throw workError(EINVAL) }
        var found = stat(), held = stat()
        let lookup = text.withCString { fstatat(parent.fileDescriptor, $0, &found, AT_SYMLINK_NOFOLLOW) }
        guard lookup == 0 else { throw workError(errno) }
        guard let r = byMarker[found.st_ino], fstat(r.marker.fileDescriptor, &held) == 0,
              found.st_dev == held.st_dev, found.st_ino == held.st_ino, held.st_nlink == 1, held.st_size == 0,
              held.st_flags == 0, held.st_uid == getuid(), (held.st_mode & 0o177777) == 0o100400
        else { throw workError(ESTALE) }
        let rc = String(decoding: r.name.data, as: UTF8.self).withCString { zxs_work_spelling(r.marker.fileDescriptor,$0) }
        guard rc == 0 else { throw workError(rc) }
        return r
    }

    func issued(_ item: FSItem) -> Row? { byItem[ObjectIdentifier(item)] }

    func rootStat() throws -> stat {
        try check(); var s = stat()
        guard fstat(parent.fileDescriptor, &s) == 0 else { throw workError(errno) }
        return s
    }

    func row(_ item: FSItem) throws -> Row {
        guard let r = byItem[ObjectIdentifier(item)], r.item === item, try resolve(r.name.data) === r
        else { throw workError(ESTALE) }
        return r
    }

    func checkOwner(_ ticket: HeldMountTicket) throws {
        try check(); try ticket.check()
        var named = stat(), held = stat()
        guard original.volumeHex == ticket.directoryID.volumeHex, ordered.allSatisfy({ $0.source.file.hasParent(ticket.directoryID) }),
              fstat(parent.fileDescriptor, &held) == 0,
              fstatat(ticket.directory.fileDescriptor, "names", &named, AT_SYMLINK_NOFOLLOW) == 0,
              held.st_dev == named.st_dev, held.st_ino == named.st_ino,
              zxs_work_spelling(parent.fileDescriptor,"names") == 0 else { throw workError(ESTALE) }
    }

    func stats() throws -> statfs {
        try check(); var s = statfs()
        guard fstatfs(parent.fileDescriptor, &s) == 0 else { throw workError(errno) }
        return s
    }
}
