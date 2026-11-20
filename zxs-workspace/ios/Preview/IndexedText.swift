import CryptoKit
import Foundation

// Bounded in-memory evidence for the pages of one already verified Store member.
struct IndexedText {
    static let blockBytes = 1 << 20
    private static let digestBytes = 32
    private let size: Int
    private let hashes: [UInt8], rewind: [UInt8], first: [UInt32]

    var count: Int { Int(first.last!) }

    private static func digest(_ bytes: [UInt8], _ count: Int) -> SHA256.Digest {
        var hash = SHA256()
        bytes.withUnsafeBytes { (buf: UnsafeRawBufferPointer) in
            hash.update(bufferPointer: UnsafeRawBufferPointer(rebasing: buf[..<count]))
        }
        return hash.finalize()
    }

    final class Build {
        private let size: Int
        private var hashes: [UInt8], rewind: [UInt8], first: [UInt32]
        private var combined = [UInt8](repeating: 0, count: IndexedText.blockBytes + 3)
        private var blocks = 0, valid = true

        init(size: Int) throws {
            try requirePreview(size > PlainText.byteLimit && size <= DirectoryIndex.maxMember)
            self.size = size
            let count = (size + IndexedText.blockBytes - 1) / IndexedText.blockBytes
            hashes = [UInt8](repeating: 0, count: count * IndexedText.digestBytes)
            rewind = [UInt8](repeating: 0, count: count + 1)
            first = [UInt32](repeating: 0, count: count + 1)
        }

        func append(_ bytes: [UInt8], count: Int, relative: Int, check: () throws -> Void) throws {
            guard valid else { return } // CRC still consumes the complete body if it isn't displayable text.
            let i = blocks
            try requirePreview(i < first.count - 1 && relative == i * IndexedText.blockBytes
                               && count == min(IndexedText.blockBytes, size - relative))
            let slot = i * IndexedText.digestBytes
            hashes.replaceSubrange(slot..<(slot + IndexedText.digestBytes), with: Array(IndexedText.digest(bytes, count)))

            let prefix = Int(rewind[i]), length = prefix + count
            combined.replaceSubrange(prefix..<length, with: bytes[..<count])
            let tail = relative + count == size ? 0 : PlainText.trailingUnit(combined, length: length)
            let real = length - tail
            try requirePreview(real > 0 && tail <= 3)
            // An autorelease pool per bounded section, not one for the potentially
            // very long verification operation. No NSString/NSRange array is retained.
            let pages: Int? = try autoreleasepool {
                guard let text = PlainText.strict(combined[..<real]) else { return nil }
                return try PlainText.scan(text, check: check)
            }
            guard let pages = pages, pages > 0 else { valid = false; return }
            guard let total = UInt32(exactly: Int(first[i]) + pages) else { throw PreviewError.format }
            first[i + 1] = total; rewind[i + 1] = UInt8(tail); blocks += 1
            for n in 0..<tail { combined[n] = combined[real + n] }
        }

        func finish() throws -> IndexedText? {
            guard valid else { return nil }
            try requirePreview(blocks == first.count - 1)
            return IndexedText(size: size, hashes: hashes, rewind: rewind, first: first)
        }
    }

    func page(_ requested: Int, bodyAt: Int, check: () throws -> Void,
              fill: (Int, UnsafeMutableRawBufferPointer) throws -> Void) throws -> String {
        try requirePreview((0..<count).contains(requested))
        var lo = 0, hi = first.count - 1 // the last value is the count, never a segment start
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2
            if Int(first[mid]) <= requested { lo = mid } else { hi = mid }
        }
        let segment = lo, head = Int(rewind[segment]), tail = Int(rewind[segment + 1])
        let n = min(Self.blockBytes, size - segment * Self.blockBytes)
        var raw = [UInt8](repeating: 0, count: Self.blockBytes)
        var bytes = [UInt8](repeating: 0, count: head + n - tail)
        func readBlock(_ i: Int, _ length: Int) throws {
            try StoredBody.chunk(bodyAt + i * Self.blockBytes, length, into: &raw, check: check, fill: fill)
            let start = i * Self.digestBytes
            try requirePreview(Self.digest(raw, length).elementsEqual(hashes[start..<(start + Self.digestBytes)]))
        }

        // Decode only the very buffers whose hashes were compared. A crossing
        // unit may take up to three bytes from an independently verified prior block.
        if head > 0 {
            try readBlock(segment - 1, Self.blockBytes)
            bytes.replaceSubrange(0..<head, with: raw[(Self.blockBytes - head)...])
        }
        try readBlock(segment, n)
        let end = bytes.count
        bytes.replaceSubrange(head..<end, with: raw[..<(n - tail)])
        return try autoreleasepool {
            guard let text = PlainText.strict(bytes[...]) else { throw PreviewError.format }
            var selected: String?
            let ordinal = requested - Int(first[segment])
            let pages = try PlainText.scan(text, check: check, emit: { i, range in
                if i == ordinal { selected = PlainText.literal(text as NSString, range) }
            })
            try requirePreview(pages == Int(first[segment + 1] - first[segment]))
            guard let selected = selected else { throw PreviewError.format }
            return selected
        }
    }
}
