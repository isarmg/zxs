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
        let r = try read(zip(["设计/选项/图纸", "空/", "设计/"], reverse: true)).root
        XCTAssertEqual(r.children?["设计"]?.children?["选项"]?.children?["图纸"]?.size, 3)
        XCTAssertEqual(r.children?["空"]?.children?.count, 0)
        XCTAssertEqual(try read(zip([])).root.children?.count, 0)
    }
    func testIndexDoesNotAuthenticateBody() throws {
        var b = zip(["x"])
        b[31] ^= 0x40
        XCTAssertEqual(try read(b).root.children?["x"]?.size, 3)
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
        XCTAssertThrowsError(try job.run(URL(fileURLWithPath: "/never-open-this.zxs"))) { error in
            guard case PreviewError.cancelled = error else { return XCTFail("unexpected result") }
        }
        let b = zip(["first", "second"])
        var checks = 0
        XCTAssertThrowsError(try DirectoryIndex.read(size: b.count, check: {
            checks += 1
            if checks == 5 { throw PreviewError.cancelled }
        }, bytes: { Array(b[$0..<($0 + $1)]) }))
    }
}
