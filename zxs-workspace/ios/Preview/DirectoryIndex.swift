import Foundation

// Displaying the declarations never reads member bodies or grants snapshot identity.
enum PreviewError: Error { case format, unavailable, cancelled }

func requirePreview(_ ok: Bool) throws {
    if !ok { throw PreviewError.format }
}

final class Member {
    let name: String
    let size: Int
    var children: [String: Member]?
    var explicit = false
    fileprivate var slot: PhysicalSlot?

    init(_ name: String, size: Int = 0, folder: Bool = true) {
        self.name = name; self.size = size
        children = folder ? [:] : nil
    }
}

fileprivate final class PhysicalSlot {
    let central: Int, centralEnd: Int, offset: Int, minimum: Int, wide: Bool
    var limit = 0
    init(_ central: Int, _ centralEnd: Int, _ offset: Int, _ minimum: Int, _ wide: Bool) {
        self.central = central; self.centralEnd = centralEnd
        self.offset = offset; self.minimum = minimum; self.wide = wide
    }
}

struct DirectoryIndex {
    static let maxSize = (256 << 30) + (32 << 20), maxMember = 128 << 30
    static let maxDirectory = 2 << 20
    static let maxSearchUnits = 128
    static let u32max = 0xffff_ffff
    let root = Member("")
    private var nameBytes = 0, nodes = 0, total = 0
    private var centralAt = 0, central: [UInt8] = [], ending: [UInt8] = []
    // Actual explicit file objects from this observation only; no pathname lookup on a later selection.
    private var files = Set<ObjectIdentifier>()

    struct NameMatch {
        let parents: [Member]  // original root through the containing directory
        let member: Member
        let ordinal: Int       // position among all siblings, not just the matches

        var path: String {
            "/" + (parents.dropFirst().map(\.name) + [member.name]).joined(separator: "/")
        }
    }

    static func orderedChildren(of directory: Member) -> [Member] {
        directory.children?.values.sorted { $0.name.utf8.lexicographicallyPrecedes($1.name.utf8) } ?? []
    }

    func findNames(_ query: String, check: () throws -> Void) throws -> [NameMatch] {
        try requirePreview(!query.isEmpty && query.utf16.count <= Self.maxSearchUnits)
        var found: [NameMatch] = []
        var parents: [Member] = []
        func visit(_ directory: Member) throws {
            try check()
            parents.append(directory)
            defer { parents.removeLast() }
            for (ordinal, member) in Self.orderedChildren(of: directory).enumerated() {
                try check()
                if (member.name as NSString).range(of: query, options: .literal).location != NSNotFound {
                    found.append(NameMatch(parents: parents, member: member, ordinal: ordinal))
                }
                if member.children != nil { try visit(member) }
            }
        }
        try visit(root)
        try check()
        return found
    }

    func originalParentRows(for match: NameMatch) -> [Member]? {
        guard match.parents.first === root, let last = match.parents.last else { return nil }
        for (parent, child) in zip(match.parents, match.parents.dropFirst()) {
            guard parent.children?.values.contains(where: { $0 === child }) == true else { return nil }
        }
        let ordered = Self.orderedChildren(of: last)
        guard ordered.indices.contains(match.ordinal), ordered[match.ordinal] === match.member else { return nil }
        return ordered
    }

    func checkMember(_ file: Member, previewText: Bool = false, check: () throws -> Void,
                     fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> MemberCRC {
        try checkMember(file, mode: previewText ? .preview : .crc, check: check, fill: fill)
    }

    struct TextIndex {
        fileprivate let root: Member, member: Member
        fileprivate let local: StoredBody.Geometry, pages: IndexedText
        var count: Int { pages.count }
    }

    private func originalSlot(_ file: Member) throws -> PhysicalSlot {
        guard file.children == nil, files.contains(ObjectIdentifier(file)), let slot = file.slot
        else { throw PreviewError.format }
        return slot
    }

    func checkMember(_ file: Member, mode: MemberReadMode, check: () throws -> Void,
                     fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> MemberCRC {
        let slot = try originalSlot(file)
        let read = try StoredBody.check(central: central, centralAt: centralAt, ending: ending,
            record: slot.central, offset: slot.offset, limit: slot.limit, centralWide: slot.wide,
            size: file.size, mode: mode, check: check, fill: fill)
        let proof = read.indexed.map { TextIndex(root: root, member: file, local: $0.0, pages: $0.1) }
        return MemberCRC(declared: read.declared, actual: read.actual, text: read.text, indexed: proof)
    }

    func textPage(_ file: Member, proof: TextIndex, page: Int, check: () throws -> Void,
                  fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> String {
        try requirePreview(proof.root === root && proof.member === file && (0..<proof.count).contains(page))
        let slot = try originalSlot(file)
        var scratch = [UInt8](repeating: 0, count: 65_536)
        func match() throws {
            try StoredBody.match(centralAt + slot.central, central[slot.central..<slot.centralEnd],
                                 scratch: &scratch, check: check, fill: fill)
            try StoredBody.match(centralAt + central.count, ending[...], scratch: &scratch, check: check, fill: fill)
            try proof.local.match(scratch: &scratch, check: check, fill: fill)
        }
        try match()
        let text = try proof.pages.page(page, bodyAt: proof.local.body, check: check, fill: fill)
        try match(); try check()
        return text
    }

    // Callers establish enclosing structure bounds before accessing fixed-width fields.
    static func u16(_ b: [UInt8], _ i: Int) -> Int { Int(b[i]) | Int(b[i + 1]) << 8 }
    static func u32(_ b: [UInt8], _ i: Int) -> Int { u16(b, i) | u16(b, i + 2) << 16 }
    static func u64(_ b: [UInt8], _ i: Int) throws -> Int {
        let high = u32(b, i + 4)
        try requirePreview(high < 0x8000_0000)
        return high << 32 | u32(b, i)
    }

    static func read(size: Int, check: () throws -> Void,
                     bytes: (Int, Int) throws -> [UInt8]) throws -> DirectoryIndex {
        try requirePreview(size >= 22 && size <= maxSize)
        func get(_ at: Int, _ length: Int) throws -> [UInt8] {
            try check()
            try requirePreview(length >= 0 && length <= maxDirectory && at >= 0 && at <= size - length)
            let b = try bytes(at, length)
            try requirePreview(b.count == length)
            return b
        }

        let window = min(65_557, size), tail = try get(size - window, window)
        var matches: Int?
        for i in stride(from: window - 22, through: 0, by: -1) {
            if u32(tail, i) == 0x06054b50 && u16(tail, i + 20) == window - i - 22 {
                try requirePreview(matches == nil); matches = i
            }
        }
        guard let e = matches else { throw PreviewError.format }
        try requirePreview(u16(tail, e + 4) == 0 && u16(tail, e + 6) == 0)
        var counts = [u16(tail, e + 8), u16(tail, e + 10), u32(tail, e + 12), u32(tail, e + 16)]
        let sentinels = [65535, 65535, u32max, u32max]
        var end = size - window + e
        let loc = end >= 20 ? try get(end - 20, 20) : []
        var ending = Array(tail[e...])
        if !loc.isEmpty && u32(loc, 0) == 0x07064b50 {
            let z = try u64(loc, 8)
            try requirePreview(u32(loc, 4) == 0 && u32(loc, 16) == 1 && end >= 76 && z == end - 76)
            let rec = try get(z, 56), declaredSize = try u64(rec, 4)
            try requirePreview(u32(rec, 0) == 0x06064b50 && declaredSize == 44 && u16(rec, 14) == 45
                               && u32(rec, 16) == 0 && u32(rec, 20) == 0)
            for i in counts.indices {
                let wide = try u64(rec, 24 + 8 * i)
                try requirePreview(counts[i] == sentinels[i] || counts[i] == wide)
                counts[i] = wide
            }
            ending = rec + loc + ending; end = z
        } else {
            for i in counts.indices { try requirePreview(counts[i] != sentinels[i]) }
        }
        let count = counts[0], length = counts[2], at = counts[3]
        try requirePreview(count == counts[1] && count <= 8192 && length <= maxDirectory
                           && at <= end && length == end - at && count <= length / 46)
        let cd = try get(at, length)
        var out = DirectoryIndex(), p = 0
        out.centralAt = at; out.central = cd; out.ending = ending
        var spans: [PhysicalSlot] = []
        for _ in 0..<count {
            try check()
            try requirePreview(cd.count - p >= 46 && u32(cd, p) == 0x02014b50)
            let host = cd[p + 5], flags = u16(cd, p + 8)
            try requirePreview([0, 3, 10].contains(host) && u16(cd, p + 6) <= 45 && flags & ~0x808 == 0
                               && u16(cd, p + 10) == 0 && u16(cd, p + 34) == 0)
            let name = p + 46, nl = u16(cd, p + 28), extra = name + nl
            let stop = extra + u16(cd, p + 30), next = stop + u16(cd, p + 32)
            try requirePreview(nl > 0 && nl <= 1024 && next <= cd.count)
            var values = [u32(cd, p + 24), u32(cd, p + 20), u32(cd, p + 42)], hasWide = false, x = extra
            while x < stop {
                try requirePreview(stop - x >= 4)
                let kind = u16(cd, x), n = u16(cd, x + 2); x += 4
                try requirePreview(n <= stop - x)
                if kind == 1 {
                    try requirePreview(!hasWide && n >= 8 && u16(cd, p + 6) == 45); hasWide = true
                    var q = x
                    for i in values.indices where values[i] == u32max {
                        try requirePreview(x + n - q >= 8); values[i] = try u64(cd, q); q += 8
                    }
                    try requirePreview(q == x + n)
                } else { try requirePreview(kind == 0x5455 || kind == 0x7875) }
                x += n
            }
            try requirePreview(hasWide || !values.contains(u32max))
            let body = values[0], off = values[2], header = 30 + nl + (flags & 8 == 0 ? 0 : 12)
            try requirePreview(body == values[1] && body <= maxMember
                               && off <= at - header && body <= at - header - off)
            let slot = PhysicalSlot(p, next, off, off + header + body, hasWide); spans.append(slot)
            let raw = cd[name..<extra]
            try requirePreview(flags & 0x800 != 0 || raw.allSatisfy { $0 < 128 })
            let folder = raw.last == 47
            guard let path = String(bytes: folder ? raw.dropLast() : raw, encoding: .utf8)
            else { throw PreviewError.format }
            let attr = u32(cd, p + 38), type = (attr >> 28) & 15
            try requirePreview(attr & 0x448 == 0 && (attr & 16 == 0 || folder)
                               && (host == 3 ? type == 0 || type == (folder ? 4 : 8) : type == 0)
                               && (!folder || body == 0))
            let row = try out.add(path: path, folder: folder, size: body, nameBytes: nl)
            if !folder { row.slot = slot; out.files.insert(ObjectIdentifier(row)) }
            p = next
        }
        try requirePreview(p == cd.count)
        var limit = at
        for slot in spans.sorted(by: { $0.offset > $1.offset }) {
            try check(); try requirePreview(slot.minimum <= limit)
            slot.limit = limit; limit = slot.offset
        }
        try check()
        return out
    }

    static func safe(_ s: String) -> Bool {
        guard !s.isEmpty, s.utf8.count <= 240,
              s.utf8.elementsEqual(s.precomposedStringWithCanonicalMapping.utf8),
              s.utf8.first != 32, s.utf8.last != 32, s.utf8.last != 46 else { return false }
        let forbidden = CharacterSet(charactersIn: "<>:\"/\\|?*")
        for u in s.unicodeScalars {
            switch u.properties.generalCategory {
            case .control, .format, .lineSeparator, .paragraphSeparator: return false
            default: if forbidden.contains(u) { return false }
            }
        }
        let base = String(decoding: s.utf8.prefix(while: { $0 != 46 }), as: UTF8.self)
            .trimmingCharacters(in: .whitespaces).uppercased()
        if ["CON","PRN","AUX","NUL","CLOCK$","CONIN$","CONOUT$"].contains(base) { return false }
        if (base.hasPrefix("COM") || base.hasPrefix("LPT")), base.count == 4,
           let last = base.last, "123456789¹²³".contains(last) { return false }
        return true
    }

    private mutating func add(path: String, folder: Bool, size: Int, nameBytes: Int) throws -> Member {
        self.nameBytes += nameBytes; total += size
        try requirePreview(self.nameBytes <= 512 * 1024 && total <= (256 << 30))
        // Split at the actual ASCII slash; grapheme-cluster segmentation is not a ZIP delimiter.
        let parts = path.utf8.split(separator: 47, omittingEmptySubsequences: false)
        try requirePreview(!parts.isEmpty && parts.count <= 48)
        var parent = root
        for (i, raw) in parts.enumerated() {
            let s = String(decoding: raw, as: UTF8.self)
            try requirePreview(Self.safe(s))
            let key = s.folding(options: .caseInsensitive, locale: Locale(identifier: "en_US_POSIX"))
            let leaf = i + 1 == parts.count, dir = !leaf || folder
            let item: Member
            if let old = parent.children?[key] {
                try requirePreview(s.utf8.elementsEqual(old.name.utf8) && (old.children != nil) == dir)
                item = old
            } else {
                nodes += 1; try requirePreview(nodes <= 12288)
                item = Member(s, size: leaf ? size : 0, folder: dir); parent.children?[key] = item
            }
            if leaf { try requirePreview(!item.explicit); item.explicit = true }
            parent = item
        }
        return parent
    }
}
