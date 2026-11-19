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
