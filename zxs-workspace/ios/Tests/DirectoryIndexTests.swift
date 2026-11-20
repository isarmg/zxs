import Foundation
import XCTest
@testable import ZipPreview

final class DirectoryIndexTests: XCTestCase {
    private func le(_ value: Int, _ width: Int) -> [UInt8] {
        (0..<width).map { UInt8(truncatingIfNeeded: value >> (8 * $0)) }
    }
    private func value(_ b: [UInt8], _ p: Int, _ n: Int) -> Int {
        (0..<n).reduce(0) { $0 | Int(b[p + $1]) << (8 * $1) }
    }
    private func zip(_ names: [String], reverse: Bool = false) -> [UInt8] {
        var out: [UInt8] = [], records: [[UInt8]] = []
        for name in names {
            let n = Array(name.utf8), dir = n.last == 47, offset = out.count
            let size = dir ? 0 : 3, crc = dir ? 0 : 0x55bc801d
            var fields = le(20, 2) + le(0x800, 2) + le(0, 2) + le(0, 4)
            fields += le(crc, 4) + le(size, 4) + le(size, 4) + le(n.count, 2) + le(0, 2)
            out += le(0x04034b50, 4) + fields + n
            if !dir { out += [1, 2, 3] }
            var central = le(0x02014b50, 4) + le(20, 2) + fields
            central += le(0, 2) + le(0, 2) + le(0, 2) + le(dir ? 16 : 0, 4) + le(offset, 4) + n
            records.append(central)
        }
        let at = out.count
        for central in (reverse ? Array(records.reversed()) : records) { out += central }
        let length = out.count - at
        out += le(0x06054b50, 4) + le(0, 4) + le(names.count, 2) + le(names.count, 2)
        out += le(length, 4) + le(at, 4) + le(0, 2)
        return out
    }
    private func read(_ b: [UInt8]) throws -> DirectoryIndex {
        try DirectoryIndex.read(size: b.count, check: {}, bytes: { Array(b[$0..<($0 + $1)]) })
    }
    func testEmptyAndNestedStoredDirectoriesInReversePhysicalOrder() throws {
        let b = zip(["设计/选项/图纸", "空/", "设计/"], reverse: true), index = try read(b)
        let r = index.root
        XCTAssertEqual(r.children?["设计"]?.children?["选项"]?.children?["图纸"]?.size, 3)
        let file = try XCTUnwrap(r.children?["设计"]?.children?["选项"]?.children?["图纸"])
        XCTAssertTrue(try verify(index, file, b).matches)
        XCTAssertEqual(r.children?["空"]?.children?.count, 0)
        XCTAssertEqual(try read(zip([])).root.children?.count, 0)
    }
    func testNameMatchesPreserveSameSpellingInSeparateParentsAndActualSecondPage() throws {
        var names = (100..<205).map { "A/item-\($0)" }
        names += ["A/readme[1].txt", "B/readme[1].txt", "C/README[1].TXT", "空目录/", "隐式/下一级/file"]
        let b = zip(names, reverse: true), index = try read(b)
        let hits = try index.findNames("readme[1]", check: {})
        XCTAssertEqual(hits.map(\.path), ["/A/readme[1].txt", "/B/readme[1].txt"])
        XCTAssertEqual(hits.map(\.ordinal), [105, 0])
        XCTAssertEqual(hits[0].ordinal / 100, 1)
        for hit in hits {
            let rows = try XCTUnwrap(index.originalParentRows(for: hit))
            XCTAssertTrue(rows[hit.ordinal] === hit.member)
            XCTAssertTrue(rows[hit.ordinal] === hit.parents.last?.children?["readme[1].txt"])
        }
        XCTAssertEqual(try index.findNames("README", check: {}).map(\.path), ["/C/README[1].TXT"])
        XCTAssertTrue(try index.findNames("A/readme", check: {}).isEmpty)  // name fragment, not a path
        XCTAssertEqual(try index.findNames("空目录", check: {}).first?.member.children?.count, 0)
        XCTAssertEqual(try index.findNames("下一级", check: {}).first?.member.explicit, false)
        let replacement = try read(b)  // Same ZIP bytes do not restore former view/node identity.
        XCTAssertNil(replacement.originalParentRows(for: hits[0]))
        let wrongLineage = DirectoryIndex.NameMatch(parents: [index.root, hits[1].parents[1]],
            member: hits[0].member, ordinal: hits[1].ordinal)
        XCTAssertNil(index.originalParentRows(for: wrongLineage))
    }

    func testNameSearchIsLiteralBoundedAndDoesNotReturnPartialResultsOnCancellation() throws {
        let max = String(repeating: "a", count: DirectoryIndex.maxSearchUnits)
        let index = try read(zip(["é", "file-a", "file-b", "file-c", max]))
        XCTAssertEqual(try index.findNames("é", check: {}).first?.member.name, "é")
        XCTAssertTrue(try index.findNames("e\u{301}", check: {}).isEmpty) // not Swift String canonical equality
        XCTAssertTrue(try index.findNames("É", check: {}).isEmpty)
        XCTAssertEqual(try index.findNames(max, check: {}).count, 1)
        XCTAssertThrowsError(try index.findNames(max + "a", check: {}))
        XCTAssertThrowsError(try index.findNames("", check: {}))
        var checks = 0
        XCTAssertThrowsError(try index.findNames("file") {
            checks += 1
            if checks == 4 { throw PreviewError.cancelled } // Already found file-a, yet no partial answer.
        }) { error in
            guard case PreviewError.cancelled = error else { return XCTFail("unexpected result") }
        }
    }

    func testIndexDoesNotAuthenticateBody() throws {
        var b = zip(["x"])
        b[31] ^= 0x40
        let observed = try read(b), file = try XCTUnwrap(observed.root.children?["x"])
        XCTAssertEqual(file.size, 3)
        XCTAssertFalse(try verify(observed, file, b).matches)
    }
    func testExactNamesAndResourceLimitRejectWholeObservation() throws {
        for names in [["Keep", "keep"], ["a", "a/b"], ["SSF", "ßf"], ["é", "e\u{301}"],
                      ["e\u{301}"], ["../x"], ["bad//leaf"], ["con.txt"], ["x\u{2028}y"]] {
            XCTAssertThrowsError(try read(zip(names)))
        }
        let b = zip(["ok"])
        XCTAssertThrowsError(try DirectoryIndex.read(size: b.count, check: {}) { _, _ in [] })
        XCTAssertThrowsError(try DirectoryIndex.read(size: DirectoryIndex.maxSize + 1, check: {}) { _, _ in
            XCTFail("oversize admitted to I/O"); return []
        })
    }
    func testImpossibleHeaderRangesAndMethodAreNotAnEmptyDirectory() {
        let original = zip(["a", "bb"])
        let cd = value(original, original.count - 6, 4), second = cd + 47
        var b = original
        b.replaceSubrange(second + 42..<second + 46, with: le(1, 4))
        XCTAssertThrowsError(try read(b))
        b.replaceSubrange(second + 42..<second + 46, with: le(cd - 33, 4))
        XCTAssertThrowsError(try read(b))
        b = original; b[cd + 10] = 8
        XCTAssertThrowsError(try read(b))
        b = original; b[b.count - 14] = 3
        XCTAssertThrowsError(try read(b))
    }
    func testFiveGiBZIP64DeclarationOnlyReadsTailAndDirectory() throws {
        let size = 5 << 30, at = size + 31
        var cd = [UInt8](repeating: 0, count: 46)
        cd.replaceSubrange(0..<4, with: le(0x02014b50, 4))
        cd[4] = 45; cd[6] = 45; cd[28] = 1; cd[30] = 20
        cd.replaceSubrange(20..<28, with: [UInt8](repeating: 255, count: 8))
        cd += [120] + le(1, 2) + le(16, 2) + le(size, 8) + le(size, 8)
        let length = cd.count, zip64 = at + length
        cd += le(0x06064b50, 4) + le(44, 8) + le(45, 2) + le(45, 2) + le(0, 8)
        cd += le(1, 8) + le(1, 8) + le(length, 8) + le(at, 8)
        cd += le(0x07064b50, 4) + le(0, 4) + le(zip64, 8) + le(1, 4)
        cd += le(0x06054b50, 4) + le(0, 4) + [UInt8](repeating: 255, count: 12) + le(0, 2)
        let eof = at + cd.count
        func sparse(_ pos: Int, _ n: Int) throws -> [UInt8] {
            try requirePreview(n <= 65_557 && pos >= eof - 65_557 && pos <= eof - n)
            return (pos..<(pos + n)).map { $0 < at ? 0 : cd[$0 - at] }
        }
        XCTAssertEqual(try DirectoryIndex.read(size: eof, check: {}, bytes: sparse).root.children?["x"]?.size, size)
        cd[length + 32] = 2
        XCTAssertThrowsError(try DirectoryIndex.read(size: eof, check: {}, bytes: sparse))
    }
    func testCancelledJobCannotOpenAnExternalDocument() {
        let job = DocumentRead(); job.cancel()
        XCTAssertThrowsError(try job.present(URL(fileURLWithPath: "/never-open-this.zxs"), onLoss: { _ in })) { error in
            guard case PreviewError.cancelled = error else { return XCTFail("unexpected result") }
        }
        let b = zip(["first", "second"])
        var checks = 0
        XCTAssertThrowsError(try DirectoryIndex.read(size: b.count, check: {
            checks += 1
            if checks == 5 { throw PreviewError.cancelled }
        }, bytes: { Array(b[$0..<($0 + $1)]) }))
    }
    private func copy(_ b: [UInt8], _ at: Int, _ dst: UnsafeMutableRawBufferPointer) throws {
        try requirePreview(at >= 0 && at <= b.count - dst.count)
        if dst.count > 0 {
            b.withUnsafeBytes { src in
                dst.baseAddress!.copyMemory(from: src.baseAddress!.advanced(by: at), byteCount: dst.count)
            }
        }
    }
    private func verify(_ index: DirectoryIndex, _ file: Member, _ b: [UInt8]) throws -> MemberCRC {
        try index.checkMember(file, check: {}, fill: { try self.copy(b, $0, $1) })
    }
    // Build a short, ordinary Store fixture; the test's independent bitwise CRC is not a product decoder.
    private func textArchive(_ payload: [UInt8]) -> [UInt8] {
        var crc = UInt32.max
        for byte in payload {
            crc ^= UInt32(byte)
            for _ in 0..<8 { crc = (crc >> 1) ^ ((crc & 1) == 0 ? 0 : 0xedb88320) }
        }
        var b = zip(["x"])
        b.replaceSubrange(31..<34, with: payload)
        let cd = 31 + payload.count
        for at in [14, cd + 16] { b.replaceSubrange(at..<(at + 4), with: le(Int(~crc), 4)) }
        for at in [18, 22, cd + 20, cd + 24] { b.replaceSubrange(at..<(at + 4), with: le(payload.count, 4)) }
        b.replaceSubrange((b.count - 6)..<(b.count - 2), with: le(cd, 4))
        return b
    }
    private func preview(_ b: [UInt8]) throws -> MemberCRC {
        let index = try read(b), file = try XCTUnwrap(index.root.children?["x"])
        return try index.checkMember(file, previewText: true, check: {}) { try self.copy(b, $0, $1) }
    }
    func testLiteralUnicodePreviewUsesTheCrcBodyOnceAndOnlyConvertsCRLF() throws {
        let sample = "读我\t𠮷 / é / e\u{301}\r\n<b>https://example.invalid</b>\n"
        let payload = Array(sample.utf8), b = textArchive(payload), index = try read(b)
        let file = try XCTUnwrap(index.root.children?["x"])
        var reads = 0
        let result = try index.checkMember(file, previewText: true, check: {}) { off, dst in
            if off == 31 { reads += 1; XCTAssertEqual(dst.count, payload.count) }
            try self.copy(b, off, dst)
        }
        XCTAssertTrue(result.matches); XCTAssertEqual(reads, 1)
        let text = try XCTUnwrap(result.text)
        XCTAssertTrue(displayed(text).utf8.elementsEqual(sample.replacingOccurrences(of: "\r\n", with: "\n").utf8))
        XCTAssertNil(try verify(index, file, b).text) // normal CRC never returns a display string
    }
    private func displayed(_ pages: PlainText.Pages) -> String {
        var result = ""
        for index in 0..<pages.count {
            let page = pages.page(index)
            XCTAssertLessThanOrEqual(page.utf16.count, PlainText.pageUnits)
            XCTAssertLessThanOrEqual(page.utf8.filter { $0 == 10 }.count, PlainText.pageNewlines)
            if let first = page.utf16.first, let last = page.utf16.last {
                XCTAssertFalse(first >= 0xdc00 && first <= 0xdfff)
                XCTAssertFalse(last >= 0xd800 && last <= 0xdbff)
            }
            result += page
        }
        return result
    }

    func testFullTextAtZeroAndBeyondThePrevious32KiBLimit() throws {
        let empty = try XCTUnwrap(preview(textArchive([])).text)
        XCTAssertTrue(empty.complete); XCTAssertEqual(empty.count, 1); XCTAssertEqual(displayed(empty), "")
        for bytes in [32 * 1024 + 1, PlainText.byteLimit] {
            let payload = [UInt8](repeating: 120, count: bytes)
            let text = try XCTUnwrap(preview(textArchive(payload)).text)
            XCTAssertTrue(text.complete); XCTAssertEqual(text.displayedBytes, bytes)
            XCTAssertGreaterThan(text.count, 1)
            XCTAssertTrue(displayed(text).utf8.elementsEqual(payload))
        }
    }

    func testPageCutsPreserveSurrogatePairsDenseLinesAndTheLastMarker() throws {
        let sample = String(repeating: "a", count: PlainText.pageUnits - 1) + "𠮷\r\n"
            + String(repeating: "row\r\n", count: PlainText.pageNewlines + 5) + "末尾 · END"
        let text = try XCTUnwrap(preview(textArchive(Array(sample.utf8))).text)
        XCTAssertEqual(text.page(0).utf16.count, PlainText.pageUnits - 1)
        XCTAssertTrue(text.page(1).hasPrefix("𠮷"))
        XCTAssertTrue(displayed(text).utf8.elementsEqual(sample.replacingOccurrences(of: "\r\n", with: "\n").utf8))
        XCTAssertTrue(text.page(text.count - 1).hasSuffix("末尾 · END"))
        // Actual viewport scrolling and VoiceOver at large type require iPhone/iPad qualification.
    }

    func testExplicitLargePrefixStillConsumesAllAndNeverIgnoresBadTailCRC() throws {
        var payload = [UInt8](repeating: 120, count: 512 * 1024)
        payload[PlainText.byteLimit] = 0x80 // first entirely unjudged text byte, after complete ASCII
        payload[payload.count - 1] = 90
        var b = textArchive(payload)
        let index = try read(b), file = try XCTUnwrap(index.root.children?["x"]), bodyEnd = 31 + payload.count
        var cursor = 31
        let result = try index.checkMember(file, previewText: true, check: {}) { off, dst in
            if off >= 31 && off < bodyEnd { XCTAssertEqual(off, cursor); cursor += dst.count }
            try self.copy(b, off, dst)
        }
        let text = try XCTUnwrap(result.text)
        XCTAssertTrue(result.matches); XCTAssertFalse(text.complete); XCTAssertEqual(cursor, bodyEnd)
        XCTAssertEqual(text.displayedBytes, PlainText.byteLimit); XCTAssertEqual(text.totalBytes, payload.count)
        XCTAssertTrue(displayed(text).utf8.elementsEqual(payload.prefix(PlainText.byteLimit)))
        b[bodyEnd - 1] ^= 1
        let bad = try preview(b)
        XCTAssertFalse(bad.matches); XCTAssertNil(bad.text) // no prefix after CRC trouble in the unshown tail
    }

    func testPrefixCutRequiresExactCrossingScalarOrCrlfEvidence() throws {
        let cap = PlainText.byteLimit
        var payload = [UInt8](repeating: 120, count: cap + 5)
        let four = Array("𠮷".utf8)
        payload.replaceSubrange((cap - 1)..<(cap - 1 + four.count), with: four)
        var text = try XCTUnwrap(preview(textArchive(payload)).text)
        XCTAssertFalse(text.complete); XCTAssertEqual(text.displayedBytes, cap - 1)
        XCTAssertTrue(displayed(text).utf8.elementsEqual(payload.prefix(cap - 1)))

        payload[cap] = 98 // invalid continuation of the scalar that starts INSIDE the budget
        var invalid = try preview(textArchive(payload))
        XCTAssertTrue(invalid.matches); XCTAssertNil(invalid.text)
        let format = Array("\u{202e}".utf8)
        payload.replaceSubrange((cap - 1)..<(cap - 1 + format.count), with: format)
        invalid = try preview(textArchive(payload))
        XCTAssertTrue(invalid.matches); XCTAssertNil(invalid.text) // valid UTF-8, rejected display profile

        payload = [UInt8](repeating: 120, count: cap + 5); payload[cap - 1] = 13; payload[cap] = 10
        text = try XCTUnwrap(preview(textArchive(payload)).text)
        XCTAssertEqual(text.displayedBytes, cap - 1)
        XCTAssertTrue(displayed(text).utf8.elementsEqual(payload.prefix(cap - 1)))
        payload[cap] = 98
        invalid = try preview(textArchive(payload))
        XCTAssertTrue(invalid.matches); XCTAssertNil(invalid.text) // an isolated CR must not be cropped away
    }
    func testOnlyValidLiteralTextWithMatchingCrcCanDisplay() throws {
        for cp in ["\u{0}", "\r", "\u{1b}", "\u{85}", "\u{202e}", "\u{2066}", "\u{34f}",
                   "\u{fe0f}", "\u{feff}", "\u{2028}", "\u{e000}"] {
            let r = try preview(textArchive(Array(("start" + cp + "!").utf8)))
            XCTAssertTrue(r.matches); XCTAssertNil(r.text)
        }
        let malformed: [[UInt8]] = [[0xc0, 0xaf], [0xe2, 0x82], [0xed, 0xa0, 0x80]]
        for bad in malformed {
            let r = try preview(textArchive(bad)); XCTAssertTrue(r.matches); XCTAssertNil(r.text)
        }
        var b = textArchive([97, 98]); b[31] = 99
        let r = try preview(b); XCTAssertFalse(r.matches); XCTAssertNil(r.text)
    }
    func testCancelledChangedAndFormerSelectionCannotReturnPreviewText() throws {
        let b = textArchive([111, 107]), doc = try read(b), other = try read(b)
        let file = try XCTUnwrap(doc.root.children?["x"])
        XCTAssertThrowsError(try other.checkMember(file, previewText: true, check: {}) { _, _ in XCTFail("former index") })
        for cancel in [false, true] {
            let job = DocumentRead(); var input = b
            XCTAssertThrowsError(try doc.checkMember(file, previewText: true, check: job.check) { off, dst in
                try self.copy(input, off, dst)
                if off == 31 {
                    if cancel { job.cancel() } else { input[input.count - 10] ^= 1 }
                }
            })
        }
    }
    func testFullIndexReadsBodyOnceAndCanJumpBackFromLastPageAcrossTheOriginalCuts() throws {
        let block = IndexedText.blockBytes, start = 31
        var body = [UInt8](repeating: 97, count: 3 * block + 15)
        let crlf = Array("\r\nNEXT".utf8), scalar = Array("𠮷NEXT".utf8), end = Array("LAST".utf8)
        body.replaceSubrange((block - 1)..<(block - 1 + crlf.count), with: crlf)
        body.replaceSubrange((2 * block - 3)..<(2 * block - 3 + scalar.count), with: scalar)
        let tail = body.count
        body.replaceSubrange((tail - end.count)..<tail, with: end)
        let b = textArchive(body), index = try read(b), item = try XCTUnwrap(index.root.children?["x"])
        var offsets: [Int] = [], sizes: [Int] = []
        func fill(_ off: Int, _ buffer: UnsafeMutableRawBufferPointer) throws {
            if (start..<(start + body.count)).contains(off) {
                XCTAssertLessThanOrEqual(off + buffer.count, start + body.count)
                offsets.append(off - start); sizes.append(buffer.count)
            }
            try copy(b, off, buffer)
        }
        let verified = try index.checkMember(item, mode: .indexed, check: {}, fill: fill)
        XCTAssertTrue(verified.matches); XCTAssertNil(verified.text)
        let proof = try XCTUnwrap(verified.indexed)
        XCTAssertEqual(offsets, [0, block, 2 * block, 3 * block])
        XCTAssertEqual(sizes, [block, block, block, 15])
        offsets = []; sizes = []
        let last = try index.textPage(item, proof: proof, page: proof.count - 1, check: {}, fill: fill)
        XCTAssertTrue(last.hasSuffix("LAST")); XCTAssertEqual(offsets, [3 * block]); XCTAssertEqual(sizes, [15])
        offsets = []; sizes = []
        let second = try index.textPage(item, proof: proof, page: 128, check: {}, fill: fill)
        XCTAssertTrue(second.hasPrefix("\nNEXT")); XCTAssertEqual(second.utf16.count, PlainText.pageUnits)
        XCTAssertEqual(offsets, [0, block]); XCTAssertEqual(sizes, [block, block])
        offsets = []
        let third = try index.textPage(item, proof: proof, page: 256, check: {}, fill: fill)
        XCTAssertTrue(third.hasPrefix("𠮷NEXT")); XCTAssertEqual(third.utf16.count, PlainText.pageUnits)
        XCTAssertEqual(offsets, [block, 2 * block])
        XCTAssertThrowsError(try index.textPage(item, proof: proof, page: -1, check: {}, fill: fill))
        XCTAssertThrowsError(try index.textPage(item, proof: proof, page: proof.count, check: {}, fill: fill))
    }

    func testIndexRejectsNonTextBeyondThePrefixAndNeverPublishesOnBadCRC() throws {
        let block = IndexedText.blockBytes
        let endings: [[UInt8]] = [[0xe2, 0x82], [13], [0xc0, 0xaf], Array("\u{202e}".utf8)]
        for end in endings {
            let b = textArchive([UInt8](repeating: 97, count: block) + end)
            let index = try read(b), item = try XCTUnwrap(index.root.children?["x"])
            let verified = try index.checkMember(item, mode: .indexed, check: {}) { try self.copy(b, $0, $1) }
            XCTAssertTrue(verified.matches); XCTAssertNil(verified.indexed)
            XCTAssertNotNil(try preview(b).text) // explicitly chosen old prefix remains distinct
        }
        var b = textArchive([UInt8](repeating: 10, count: block + 7))
        let index = try read(b), item = try XCTUnwrap(index.root.children?["x"])
        let result = try index.checkMember(item, mode: .indexed, check: {}) { try self.copy(b, $0, $1) }
        let proof = try XCTUnwrap(result.indexed)
        XCTAssertEqual(proof.count, block / PlainText.pageNewlines + 1)
        let dense = try index.textPage(item, proof: proof, page: 4000, check: {}) { try self.copy(b, $0, $1) }
        let last = try index.textPage(item, proof: proof, page: proof.count - 1, check: {}) { try self.copy(b, $0, $1) }
        XCTAssertEqual(dense, String(repeating: "\n", count: PlainText.pageNewlines))
        XCTAssertEqual(last, String(repeating: "\n", count: 7))
        b[31 + block + 6] = 90 // valid text, wrong CRC, later than the old preview prefix
        let changed = try index.checkMember(item, mode: .indexed, check: {}) { try self.copy(b, $0, $1) }
        XCTAssertFalse(changed.matches); XCTAssertNil(changed.indexed); XCTAssertNil(changed.text)
    }

    func testIndexedPageRejectsFormerSelectionChangedExactLayoutAndUnshownBytesInTouchedBlocks() throws {
        let block = IndexedText.blockBytes, start = 31
        var body = [UInt8](repeating: 97, count: 2 * block + 7)
        body[block - 1] = 13; body[block] = 10
        var b = textArchive(body)
        let index = try read(b), other = try read(b), item = try XCTUnwrap(index.root.children?["x"])
        let result = try index.checkMember(item, mode: .indexed, check: {}) { try self.copy(b, $0, $1) }
        let proof = try XCTUnwrap(result.indexed), otherItem = try XCTUnwrap(other.root.children?["x"])
        XCTAssertThrowsError(try other.textPage(otherItem, proof: proof, page: 128, check: {}) {
            _, _ in XCTFail("old proof reached a different selection")
        })
        for offset in [start - 1, b.count - 23, b.count - 1, start, start + 2 * block - 1] {
            let original = b[offset]; b[offset] ^= 1
            XCTAssertThrowsError(try index.textPage(item, proof: proof, page: 128, check: {}) { try self.copy(b, $0, $1) })
            b[offset] = original
        }
        var late = b
        XCTAssertThrowsError(try index.textPage(item, proof: proof, page: 128, check: {}) { off, out in
            try self.copy(late, off, out)
            if off == start + block { late[late.count - 1] ^= 1 }
        })
        for initial in [false, true] {
            let job = DocumentRead()
            func stop(_ off: Int, _ out: UnsafeMutableRawBufferPointer) throws {
                try copy(b, off, out)
                if off == start + block { job.cancel() }
            }
            if initial {
                XCTAssertThrowsError(try index.checkMember(item, mode: .indexed, check: job.check, fill: stop))
            } else {
                XCTAssertThrowsError(try index.textPage(item, proof: proof, page: 128, check: job.check, fill: stop))
            }
        }
    }

    // Forced small ZIP64 records and an offset-only central extra with an ordinary local version.
    private func streamed(empty: Bool, wide: Bool, offsetOnly: Bool, signed: Bool,
                          narrowOffsetDescriptor: Bool = false, origin: Int = 0) -> [UInt8] {
        let bytes: [UInt8] = empty ? [] : [1, 2, 3], crc = empty ? 0 : 0x55bc801d, size = bytes.count
        let name = Array("x".utf8), localWide = wide && !offsetOnly
        let v = localWide ? 45 : (narrowOffsetDescriptor ? 20 : 10)
        let fieldWidth = wide && !narrowOffsetDescriptor ? 8 : 4
        var x: [UInt8] = []
        if localWide { x = le(1, 2) + le(16, 2) + le(0, 8) + le(0, 8) }
        let counts = localWide ? [UInt8](repeating: 255, count: 8) : le(0, 8)
        var b = le(0x04034b50, 4) + le(v, 2) + le(0x808, 2) + le(0, 2) + le(0, 4)
        b += le(0, 4) + counts + le(name.count, 2) + le(x.count, 2) + name + x
        b += bytes
        if signed { b += le(0x08074b50, 4) }
        b += le(crc, 4) + le(size, fieldWidth) + le(size, fieldWidth)
        let at = origin + b.count, old = offsetOnly ? 0xffff_ffff : origin
        if wide {
            x = offsetOnly ? le(1, 2) + le(8, 2) + le(origin, 8) : le(1, 2) + le(16, 2) + le(size, 8) + le(size, 8)
        }
        let centralSizes = localWide ? [UInt8](repeating: 255, count: 8) : le(size, 4) + le(size, 4)
        var central = le(0x02014b50, 4) + le(20, 2) + le(wide ? 45 : 10, 2)
        central += le(0x808, 2) + le(0, 2) + le(0, 4) + le(crc, 4) + centralSizes
        central += le(name.count, 2) + le(x.count, 2) + le(0, 2) + le(0, 2) + le(0, 2) + le(0, 4)
        central += le(old, 4) + name + x
        b += central
        if at > DirectoryIndex.u32max {
            let z = origin + b.count
            b += le(0x06064b50, 4) + le(44, 8) + le(45, 2) + le(45, 2) + le(0, 8)
            b += le(1, 8) + le(1, 8) + le(central.count, 8) + le(at, 8)
            b += le(0x07064b50, 4) + le(0, 4) + le(z, 8) + le(1, 4)
        }
        b += le(0x06054b50, 4) + le(0, 4) + le(1, 2) + le(1, 2)
        b += le(central.count, 4) + le(min(at, DirectoryIndex.u32max), 4) + le(0, 2)
        return b
    }
    func testDescriptorWidthsAndOptionalSignaturesIncludingEmptyFilesAndOffsetOnlyZIP64() throws {
        for (wide, offsetOnly) in [(false, false), (true, false), (true, true)] {
            for empty in [false, true] { for signed in [false, true] {
                let b = streamed(empty: empty, wide: wide, offsetOnly: offsetOnly, signed: signed)
                let doc = try read(b), file = try XCTUnwrap(doc.root.children?["x"])
                let result = try verify(doc, file, b)
                XCTAssertTrue(result.matches); XCTAssertEqual(result.actual, empty ? 0 : 0x55bc801d)
                let cd = value(b, b.count - 6, 4)
                var bad = b; bad[cd - 1] ^= 1 // full tuple, not a matching short prefix
                XCTAssertThrowsError(try verify(doc, file, bad))
                if wide { var low = b; low[cd + 6] = 10; XCTAssertThrowsError(try read(low)) }
            }}
        }
    }
    func testLargeOffsetOnlyZIP64OrdinaryLocalHeaderAndNarrowDescriptor() throws {
        // CPython's nonseekable writer can emit this: local v2.0, small Store data, then
        // a narrow descriptor; only the *later* central directory needs the 64-bit offset.
        let origin = (1 << 32) + 37
        for empty in [false, true] { for signed in [false, true] {
            var tail = streamed(empty: empty, wide: true, offsetOnly: true, signed: signed,
                                narrowOffsetDescriptor: true, origin: origin)
            let eof = origin + tail.count
            func sparse(_ pos: Int, _ n: Int) throws -> [UInt8] {
                try requirePreview(n >= 0 && n <= 65_557 && pos >= eof - 65_557 && pos <= eof - n)
                return (pos..<(pos + n)).map { $0 < origin ? 0 : tail[$0 - origin] }
            }
            let doc = try DirectoryIndex.read(size: eof, check: {}, bytes: sparse)
            let file = try XCTUnwrap(doc.root.children?["x"])
            func verifyOne() throws -> MemberCRC {
                try doc.checkMember(file, check: {}) { off, dst in try self.copy(sparse(off, dst.count), 0, dst) }
            }
            XCTAssertTrue(try verifyOne().matches)
            let lastSizeByte = 31 + (empty ? 0 : 3) + (signed ? 16 : 12) - 1
            tail[lastSizeByte] ^= 1
            XCTAssertThrowsError(try verifyOne()) // malformed tuple is no result, not a payload mismatch
        }}
    }
    func testSignatureValueAlsoWorksAsCRCButAmbiguousFramingRefuses() throws {
        let sig = 0x08074b50
        XCTAssertEqual(try StoredBody.descriptorLength(le(sig, 4) + le(7, 4) + le(7, 4),
            wide: false, crc: sig, size: 7), 12)
        XCTAssertEqual(try StoredBody.descriptorLength(le(sig, 4) + le(sig, 4) + le(7, 4) + le(7, 4),
            wide: false, crc: sig, size: 7), 16)
        XCTAssertThrowsError(try StoredBody.descriptorLength(Array(repeating: le(sig, 4), count: 4).flatMap { $0 },
            wide: false, crc: sig, size: sig))
    }
    func testMemberFromAnotherObservationAndDirectoryRefuseBeforeIO() throws {
        let b = zip(["d/x"]), a = try read(b), other = try read(b)
        let d = try XCTUnwrap(a.root.children?["d"]), x = try XCTUnwrap(d.children?["x"])
        for item in [x, d] {
            XCTAssertThrowsError(try other.checkMember(item, check: {}) { _, _ in XCTFail("wrong item admitted") })
        }
    }
    func testNeverReadPastNextHeaderAndRejectLocalNameOrDirectoryChangedWhileReading() throws {
        let b = zip(["a", "bb"]), index = try read(b), file = try XCTUnwrap(index.root.children?["a"])
        var wrong = b; wrong[28] = 50 // local extra length intrudes into bb
        XCTAssertThrowsError(try verify(index, file, wrong))
        for pos in [30, value(b, b.count - 6, 4) + 16, b.count - 2] {
            var bytes = b
            XCTAssertThrowsError(try index.checkMember(file, check: {}) { off, target in
                try self.copy(bytes, off, target)
                if off == 31 { bytes[pos] ^= 1 } // a CRC match cannot override changed headers
            })
        }
    }
    func testCancelledOrTruncatedMemberReadNeverReturnsAResult() throws {
        let b = zip(["a"]), index = try read(b), file = try XCTUnwrap(index.root.children?["a"])
        for cancel in [true, false] {
            let job = DocumentRead()
            XCTAssertThrowsError(try index.checkMember(file, check: job.check) { off, target in
                if off == 31 {
                    if cancel { job.cancel(); try job.check() }
                    throw PreviewError.unavailable
                }
                try self.copy(b, off, target)
            })
        }
    }

}
