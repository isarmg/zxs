// Source-only native qualification. The harness supplies a fresh *disposable* private
// APFS namespace and original WorkFiles for this exact five-file catalog. No entry point.
import Darwin
import Foundation
import FSKit

@available(macOS 27.0, *)
enum FSKitNamesQualification {
    static let ids = (1...5).map { String(format: "%032x", $0) }
    static var spellings: [Data] {
        [Data("A.txt".utf8), Data("a.txt".utf8), Data(("a-" + ids[1] + ".txt").utf8),
         Data([0x65, 0xcc, 0x81, 0x2e, 0x74, 0x78, 0x74]), Data([0xc3, 0xa9, 0x2e, 0x74, 0x78, 0x74])]
    }

    static func verify(_ index: NameIndex, expectedSensitive: Bool) throws {
        func require(_ value: Bool) throws { if !value { throw workError(EIO) } }
        try require(index.ordered.count == ids.count && index.caseFormat ==
                    (expectedSensitive ? .sensitive : .insensitiveCasePreserving))
        func registered(_ position: Int) throws -> NameIndex.Row {
            guard let row = index.ordered.first(where: { $0.source.containerID == ids[position] }),
                  row.source.remoteName == spellings[position], try index.row(row.item) === row
            else { throw workError(ESTALE) }
            return row
        }
        let one = try registered(0), two = try registered(1), literal = try registered(2)
        let decomposed = try registered(3), composed = try registered(4)
        try require(one.name.data == spellings[0] && literal.name.data == spellings[2])
        let foundOne = try index.resolve(spellings[0]), foundTwo = try index.resolve(spellings[1])
        try require(foundOne === one && foundTwo === (expectedSensitive ? two : one))
        let alias = Data(("a-" + ids[1] + "-1.txt").utf8)
        try require(two.name.data == (expectedSensitive ? spellings[1] : alias))
        let foundLiteral = try index.resolve(spellings[2])
        try require(foundLiteral === literal)
        // Both APFS variants preserve original bytes but treat canonically equivalent names as one.
        let resolvedNFC = try index.resolve(spellings[4]), resolvedNFD = try index.resolve(spellings[3])
        try require(resolvedNFC === decomposed && resolvedNFD === decomposed && decomposed.name.data == spellings[3])
        try require(composed.name.data != spellings[3] && composed.name.data != spellings[4])
        for r in index.ordered {
            let actual = try index.resolve(r.name.data)
            try require(actual === r)
        }
    }

    // Harness plants a zero-length *unissued* occupant in this disposable index after
    // verify(), never a user file. It must not become visible or displace any registered item.
    static func rejectPlanted(_ index: NameIndex, at bytes: Data) throws {
        guard NameIndex.component(bytes) else { throw workError(EINVAL) }
        do { _ = try index.resolve(bytes) }
        catch let e as NSError where e.domain == NSPOSIXErrorDomain && e.code == Int(ESTALE) { return }
        throw workError(EIO)
    }
}
