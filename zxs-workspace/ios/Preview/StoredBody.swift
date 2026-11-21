import Foundation
import CryptoKit

enum MemberReadMode: Equatable { case crc, preview, indexed, snapshot }

struct MemberCRC {
    let declared: UInt32, actual: UInt32
    let text: PlainText.Pages?
    let indexed: DirectoryIndex.TextIndex?
    var matches: Bool { declared == actual }
}

enum PlainText {
    static let byteLimit = 256 * 1024
    static let pageUnits = 8 * 1024, pageNewlines = 256

    struct Pages {
        let displayedBytes: Int, totalBytes: Int // original UTF-8 before CRLF presentation normalization
        private let text: NSString
        private let ranges: [NSRange]

        fileprivate init(_ value: String, displayed: Int, total: Int, ranges: [NSRange]) {
            text = value as NSString; self.ranges = ranges
            displayedBytes = displayed; totalBytes = total
        }

        var complete: Bool { displayedBytes == totalBytes }
        var count: Int { ranges.count }
        func page(_ index: Int) -> String { PlainText.literal(text, ranges[index]) }
    }

    static func captureLength(_ total: Int) -> Int { min(total, byteLimit + 3) }

    private static func scalarWidth(_ first: UInt8) -> Int {
        switch first {
        case 0xc2...0xdf: return 2
        case 0xe0...0xef: return 3
        case 0xf0...0xf4: return 4
        default: return 1
        }
    }

    // A nonfinal raw block may leave one scalar or CRLF for the following segment.
    static func trailingUnit(_ bytes: [UInt8], length: Int) -> Int {
        guard length > 0 else { return 0 }
        if bytes[length - 1] == 13 { return 1 }
        var start = length - 1
        let floor = max(0, length - 4)
        while start > floor && bytes[start] & 0xc0 == 0x80 { start -= 1 }
        return scalarWidth(bytes[start]) > length - start ? length - start : 0
    }

    static func strict(_ bytes: ArraySlice<UInt8>) -> String? {
        guard let value = String(bytes: bytes, encoding: .utf8),
              value.utf8.elementsEqual(bytes) else { return nil }
        return value
    }

    private static func permitted(_ scalar: Unicode.Scalar) -> Bool {
        let value = scalar.value
        if value == 9 || value == 10 || (32...126).contains(value) { return true }
        guard !scalar.properties.isDefaultIgnorableCodePoint else { return false }
        switch scalar.properties.generalCategory {
        case .control, .format, .lineSeparator, .paragraphSeparator, .privateUse, .unassigned, .surrogate: return false
        default: return true
        }
    }

    // Count pages without constructing a table for large members. NSRange
    // uses the raw UTF-16 positions; budgets count CRLF as one visible LF.
    static func scan(_ text: String, check: () throws -> Void,
                     emit: ((Int, NSRange) -> Void)? = nil) throws -> Int? {
        var start = 0, raw = 0, units = 0, lines = 0, count = 0, pendingCR = false
        func cut() throws {
            try check()
            emit?(count, NSRange(location: start, length: raw - start))
            count += 1; start = raw; units = 0; lines = 0
        }
        try check()
        for scalar in text.unicodeScalars {
            let cp = scalar.value, shown = cp > 0xffff ? 2 : 1
            var input = shown
            if pendingCR {
                guard cp == 10 else { return nil }
                pendingCR = false; input = 2
            } else if cp == 13 {
                pendingCR = true; continue
            } else if !permitted(scalar) { return nil }
            if units + shown > pageUnits { try cut() }
            raw += input; units += shown
            if cp == 10 {
                lines += 1
                if lines == pageNewlines { try cut() }
            }
        }
        guard !pendingCR else { return nil }
        if raw != start || count == 0 { try cut() }
        return count
    }

    static func literal(_ text: NSString, _ range: NSRange) -> String {
        text.substring(with: range).replacingOccurrences(of: "\r\n", with: "\n", options: .literal)
    }

    static func decode(_ bytes: [UInt8], total: Int, check: () throws -> Void) throws -> Pages? {
        guard total >= 0, bytes.count == captureLength(total) else { return nil }
        var kept = min(total, byteLimit), checked = kept
        if total > byteLimit {
            let crossing = trailingUnit(bytes, length: kept)
            if crossing > 0 {
                if bytes[kept - 1] == 13 {
                    // Do not discard an isolated CR at the budget boundary.
                    guard kept < bytes.count, bytes[kept] == 10 else { return nil }
                    kept -= 1; checked = kept
                } else {
                    kept -= crossing; checked = kept + scalarWidth(bytes[kept])
                    guard checked <= bytes.count else { return nil }
                }
            }
        }
        guard var text = strict(bytes[..<checked]) else { return nil }
        if checked > kept {
            guard let last = text.unicodeScalars.last, permitted(last),
                  let prefix = strict(bytes[..<kept]) else { return nil }
            text = prefix
        }
        var ranges: [NSRange] = []
        guard try scan(text, check: check, emit: { _, range in ranges.append(range) }) != nil else { return nil }
        return Pages(text, displayed: kept, total: total, ranges: ranges)
    }
}

// One deliberately selected ordinary Store member, never a whole-archive or version credential.
enum StoredBody {
    struct Geometry {
        let offset: Int, body: Int, end: Int
        let header: [UInt8], fields: [UInt8], descriptor: [UInt8]

        func match(scratch: inout [UInt8], check: () throws -> Void,
                   fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws {
            try StoredBody.match(offset, header[...], scratch: &scratch, check: check, fill: fill)
            try StoredBody.match(offset + 30, fields[...], scratch: &scratch, check: check, fill: fill)
            try StoredBody.match(end, descriptor[...], scratch: &scratch, check: check, fill: fill)
        }
    }

    struct Checked {
        let declared: UInt32, actual: UInt32
        let text: PlainText.Pages?
        let indexed: (Geometry, IndexedText)?
        let sha256: String?
    }

    static func chunk(_ at: Int, _ length: Int, into block: inout [UInt8], check: () throws -> Void,
                      fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws {
        try check()
        try block.withUnsafeMutableBytes { (raw: UnsafeMutableRawBufferPointer) in
            try fill(at, UnsafeMutableRawBufferPointer(rebasing: raw[..<length]))
        }
    }

    static func match(_ at: Int, _ expected: ArraySlice<UInt8>, scratch: inout [UInt8], check: () throws -> Void,
                      fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws {
        var i = 0
        while i < expected.count {
            let n = min(scratch.count, expected.count - i), start = expected.startIndex + i
            try chunk(at + i, n, into: &scratch, check: check, fill: fill)
            try requirePreview(scratch[..<n].elementsEqual(expected[start..<(start + n)]))
            i += n
        }
    }

    static func check(central cd: [UInt8], centralAt: Int, ending: [UInt8], record p: Int, offset: Int,
                      limit: Int, centralWide: Bool, size: Int, mode: MemberReadMode, check: () throws -> Void,
                      progress: (Int) -> Void = { _ in },
                      fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> Checked {
        let u16 = DirectoryIndex.u16, u32 = DirectoryIndex.u32, max32 = DirectoryIndex.u32max
        try requirePreview(size >= 0 && size <= DirectoryIndex.maxMember)
        var scratch = [UInt8](repeating: 0, count: 65_536)
        func take(_ at: Int, _ count: Int) throws -> [UInt8] {
            try check(); try requirePreview(count >= 0 && count <= 65_535 + 1024 && at >= offset && at <= limit - count)
            var out = [UInt8](repeating: 0, count: count)
            if count > 0 { try out.withUnsafeMutableBytes { try fill(at, $0) } }
            return out
        }
        func checkDirectory() throws {
            try match(centralAt, cd[...], scratch: &scratch, check: check, fill: fill)
            try match(centralAt + cd.count, ending[...], scratch: &scratch, check: check, fill: fill)
        }
        // A whole-snapshot caller compares the entire index once on either
        // side of all members. Ordinary isolated CRC checks keep their own gates.
        if mode != .snapshot { try checkDirectory() }
        let flags = u16(cd, p + 8), crc = u32(cd, p + 16), nl = u16(cd, p + 28), streaming = flags & 8 != 0
        let h = try take(offset, 30), version = u16(h, 4), name = u16(h, 26), extra = u16(h, 28)
        try requirePreview(u32(h, 0) == 0x04034b50 && (10...45).contains(version)
            && (version == u16(cd, p + 6) || (u16(cd, p + 6) == 45 && u32(cd, p + 42) == max32))
            && u16(h, 6) == flags && u16(h, 8) == 0 && u32(h, 10) == u32(cd, p + 12) && name == nl)
        let fields = try take(offset + 30, name + extra), body = offset + 30 + fields.count
        try requirePreview(fields[..<name].elementsEqual(cd[(p + 46)..<(p + 46 + name)]) && size <= limit - body)
        var packed = u32(h, 18), plain = u32(h, 22), seenWide = false, x = name
        let wide = packed == max32 || plain == max32
        while x < fields.count {
            try requirePreview(fields.count - x >= 4)
            let tag = u16(fields, x), n = u16(fields, x + 2); x += 4
            try requirePreview(n <= fields.count - x)
            if tag == 1 {
                try requirePreview(!seenWide && packed == max32 && plain == max32 && n == 16 && version >= 45)
                seenWide = true; plain = try DirectoryIndex.u64(fields, x); packed = try DirectoryIndex.u64(fields, x + 8)
            } else { try requirePreview(tag == 0x5455 || tag == 0x7875) }
            x += n
        }
        try requirePreview(wide == seenWide && u32(h, 14) == (streaming ? 0 : crc)
            && plain == (streaming ? 0 : size) && packed == plain)
        let end = body + size
        var descriptor: [UInt8] = []
        if streaming {
            // A ZIP64 offset alone says nothing about the streamed member's two size fields.
            // Use its complete physical span, not a prefix that also happens to fit.
            let length = limit - end
            let sizeWide = wide || u32(cd, p + 20) == max32 || u32(cd, p + 24) == max32
            let offsetOnly = centralWide && !sizeWide && u32(cd, p + 42) == max32
            let wideDD = sizeWide || (offsetOnly && (length == 20 || length == 24))
            let base = wideDD ? 20 : 12
            try requirePreview(length == base || length == base + 4)
            descriptor = try take(end, length)
            try requirePreview(try descriptorLength(descriptor, wide: wideDD, crc: crc, size: size) == length)
        }
        let local = Geometry(offset: offset, body: body, end: end, header: h, fields: fields, descriptor: descriptor)
        let captureLimit = mode == .preview ? PlainText.captureLength(size) : 0
        var capture: [UInt8]? = mode == .preview ? [] : nil
        capture?.reserveCapacity(captureLimit) // CRC-only never allocates a text buffer
        let builder = try (mode == .indexed ? IndexedText.Build(size: size) : nil)
        var block = [UInt8](repeating: 0, count: builder == nil ? 65_536 : IndexedText.blockBytes)
        var sum = crc32(0, nil, 0), pos = body
        var hash = mode == .snapshot ? SHA256() : nil
        while pos < end {
            let n = min(block.count, end - pos)
            try chunk(pos, n, into: &block, check: check, fill: fill)
            sum = block.withUnsafeBufferPointer { crc32(sum, $0.baseAddress, uInt(n)) }
            if hash != nil {
                block.withUnsafeBytes { raw in hash?.update(bufferPointer: UnsafeRawBufferPointer(rebasing: raw[..<n])) }
            }
            let relative = pos - body, beforeBudget = captureLimit - relative
            if beforeBudget > 0 { capture?.append(contentsOf: block[..<min(n, beforeBudget)]) }
            try builder?.append(block, count: n, relative: relative, check: check)
            pos += n
            progress(n) // neither ZIP metadata nor a later text-page read
        }
        try local.match(scratch: &scratch, check: check, fill: fill)
        if mode != .snapshot { try checkDirectory() }
        try check()
        let expected = UInt32(crc), actual = UInt32(sum)
        var text: PlainText.Pages? = nil, indexed: (Geometry, IndexedText)? = nil
        if expected == actual {
            if let capture = capture { text = try PlainText.decode(capture, total: size, check: check) }
            if let pages = try builder?.finish() { indexed = (local, pages) }
        }
        try check()
        let sha256 = hash.map { $0.finalize().map { String(format: "%02x", $0) }.joined() }
        return Checked(declared: expected, actual: actual, text: text, indexed: indexed, sha256: sha256)
    }

    static func descriptorLength(_ d: [UInt8], wide: Bool, crc: Int, size: Int) throws -> Int {
        let u32 = DirectoryIndex.u32, base = wide ? 20 : 12
        func tuple(_ p: Int) -> Bool {
            guard d.count - p >= base, u32(d, p) == crc else { return false }
            if !wide { return u32(d, p + 4) == size && u32(d, p + 8) == size }
            let lo = size & DirectoryIndex.u32max, hi = size >> 32
            return u32(d, p + 4) == lo && u32(d, p + 8) == hi && u32(d, p + 12) == lo && u32(d, p + 16) == hi
        }
        let bare = tuple(0), signed = d.count >= base + 4 && u32(d, 0) == 0x08074b50 && tuple(4)
        try requirePreview(bare != signed) // CRC itself can be the signature; don't guess an ambiguous framing
        return signed ? base + 4 : base
    }
}
