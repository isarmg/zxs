import Foundation
import XCTest
@testable import ZipPreview

final class WholeFileStageTests: XCTestCase {
    private enum Stop: Error { case cancelled, sourceChanged }

    func testStreamsPastTextLimitAndCannotReplaceOriginalFrozenBytes() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let data = Data((0..<(300 * 1024 + 1)).map { UInt8(truncatingIfNeeded: $0) })
        let id = UUID()
        var largest = 0
        let staged = try store.freeze(id: id, length: data.count, check: {}, fill: { offset, buffer in
            largest = max(largest, buffer.count)
            _ = data.copyBytes(to: buffer, from: offset..<(offset + buffer.count))
        }, validate: {})
        XCTAssertEqual(largest, 65_536)
        XCTAssertThrowsError(try store.freeze(id: id, length: 0, check: {}, fill: { _, _ in
            XCTFail("empty body has no reads")
        }, validate: {}))
        let held = try WholeFileStage(directory: directory).reopen(staged, check: {})
        defer { try? held.close() }
        XCTAssertEqual(try held.readToEnd(), data)
    }

    func testDraftNeverPublishesWhenSourceInvalidatesOrCopyCancels() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let revoked = UUID()
        XCTAssertThrowsError(try store.freeze(id: revoked, length: 0, check: {}, fill: { _, _ in
            XCTFail("empty body has no reads")
        }, validate: { throw Stop.sourceChanged }))
        let cancelled = UUID()
        XCTAssertThrowsError(try store.freeze(id: cancelled, length: 1, check: {}, fill: { _, _ in
            throw Stop.cancelled
        }, validate: {}))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path), [])
    }

    func testChangedFrozenBytesAndWrongMetadataDoNotReopen() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let id = UUID()
        let exact = try store.freeze(id: id, length: 3, check: {}, fill: { _, buffer in
            _ = Data("abc".utf8).copyBytes(to: buffer)
        }, validate: {})
        XCTAssertEqual(exact.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        let wrong = WholeFileStage.Identity(id: id, length: 2, sha256: exact.sha256)
        XCTAssertThrowsError(try store.reopen(wrong, check: {}))
        let name = directory.appendingPathComponent(id.uuidString)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: name.path)
        try Data("xyz".utf8).write(to: name)
        try FileManager.default.setAttributes([.posixPermissions: 0o400], ofItemAtPath: name.path)
        XCTAssertThrowsError(try store.reopen(exact, check: {}))
    }

    func testCompletedStagingDoesNotPublishBeforeFinalAuthorityCheck() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let id = UUID()
        let pending = try store.prepare(id: id, length: 0, check: {}, fill: { _, _ in
            XCTFail("empty body has no reads")
        }, validate: {})
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent(id.uuidString).path))
        XCTAssertThrowsError(try store.publish(pending, check: { throw Stop.cancelled }))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent(id.uuidString).path))
    }

    func testReopenRejectsProtectionDowngradeEvenWithIdenticalBytes() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let id = UUID()
        let exact = try store.freeze(id: id, length: 0, check: {}, fill: { _, _ in }, validate: {})
        let name = directory.appendingPathComponent(id.uuidString)
        try FileManager.default.setAttributes([.protectionKey: FileProtectionType.none], ofItemAtPath: name.path)
        XCTAssertThrowsError(try store.reopen(exact, check: {}))
    }
}
