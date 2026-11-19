import Foundation

struct MemberCRC {
    let declared: UInt32, actual: UInt32
    let text: String? // nil for CRC-only, mismatch or an unsupported text profile; "" is valid
    var matches: Bool { declared == actual }
}

enum PlainText {
    static let byteLimit = 32 * 1024
    static func decode(_ bytes: [UInt8]) -> String? {
        guard bytes.count <= byteLimit, let text = String(bytes: bytes, encoding: .utf8),
              text.utf8.elementsEqual(bytes) else { return nil } // never repair malformed bytes
        var cr = false, hasCR = false
        for scalar in text.unicodeScalars {
            if cr {
                guard scalar.value == 10 else { return nil }
                cr = false; continue
            }
            switch scalar.value {
            case 9, 10: continue
            case 13: cr = true; hasCR = true; continue
            default: break
            }
            guard !scalar.properties.isDefaultIgnorableCodePoint else { return nil }
            switch scalar.properties.generalCategory {
            case .control, .format, .lineSeparator, .paragraphSeparator, .privateUse, .unassigned, .surrogate: return nil
            default: break
            }
        }
        guard !cr else { return nil }
        return hasCR ? text.replacingOccurrences(of: "\r\n", with: "\n", options: .literal) : text
    }
}

// One deliberately selected ordinary Store member, never a whole-archive or version credential.
enum StoredBody {
    static func check(central cd: [UInt8], centralAt: Int, ending: [UInt8], record p: Int, offset: Int,
                      limit: Int, centralWide: Bool, size: Int, previewText: Bool, check: () throws -> Void,
                      fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> MemberCRC {
        let u16 = DirectoryIndex.u16, u32 = DirectoryIndex.u32, max32 = DirectoryIndex.u32max
        try requirePreview(!previewText || (0...PlainText.byteLimit).contains(size))
        var block = [UInt8](repeating: 0, count: 65_536)
        func take(_ at: Int, _ count: Int) throws -> [UInt8] {
            try check(); try requirePreview(count >= 0 && count <= 65_535 + 1024 && at >= offset && at <= limit - count)
            var out = [UInt8](repeating: 0, count: count)
            if count > 0 { try out.withUnsafeMutableBytes { try fill(at, $0) } }
            return out
        }
        func chunk(_ at: Int, _ count: Int) throws {
            try check()
            try block.withUnsafeMutableBytes { (raw: UnsafeMutableRawBufferPointer) in
                try fill(at, UnsafeMutableRawBufferPointer(rebasing: raw[..<count]))
            }
        }
        func match(_ at: Int, _ expected: [UInt8]) throws {
            var i = 0
            while i < expected.count {
                let n = min(block.count, expected.count - i)
                try chunk(at + i, n)
                try requirePreview(block[..<n].elementsEqual(expected[i..<(i + n)]))
                i += n
            }
        }
        func checkDirectory() throws { try match(centralAt, cd); try match(centralAt + cd.count, ending) }
        try checkDirectory()
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
        var capture: [UInt8]? = previewText ? [] : nil
        capture?.reserveCapacity(size) // no allocation on the original CRC-only path
        var sum = crc32(0, nil, 0), pos = body
        while pos < end {
            let n = min(block.count, end - pos)
            try chunk(pos, n)
            sum = block.withUnsafeBufferPointer { crc32(sum, $0.baseAddress, uInt(n)) }
            capture?.append(contentsOf: block[..<n])
            pos += n
        }
        try check(); try match(offset, h); try match(offset + 30, fields); try match(end, descriptor)
        try checkDirectory(); try check()
        let expected = UInt32(crc), actual = UInt32(sum)
        let text = expected == actual ? capture.flatMap(PlainText.decode) : nil
        try check()
        return MemberCRC(declared: expected, actual: actual, text: text)
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
