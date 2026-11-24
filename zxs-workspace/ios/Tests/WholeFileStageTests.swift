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

    func testPostLinkFailureRetainsOriginalOperationAndCanOpenAfterSync() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = WholeFileStage(directory: directory)
        let id = UUID()
        let candidate = try store.prepare(id: id, length: 1, check: {}, fill: { _, buffer in
            buffer[0] = 0x80
        }, validate: {})
        XCTAssertThrowsError(try store.publish(candidate, check: {}, afterLink: { throw Stop.sourceChanged })) {
            guard case WholeFileStage.Failure.retained(let identity) = $0 else { return XCTFail("expected retained identity") }
            XCTAssertEqual(identity, candidate.identity)
        }
        let opened = try await store.openForUpload(candidate.identity)
        XCTAssertEqual(try opened.readChunk(maximum: 1), Data([0x80]))
        XCTAssertEqual(try opened.readChunk(maximum: 1)?.count ?? 0, 0)
    }

    func testIntentWithoutPublishedBodyRemainsRecoverableButCannotUpload() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let files = WholeFileStage(directory: root.appendingPathComponent("files"))
        let intents = RemoteSaveStore(directory: root.appendingPathComponent("intents"))
        let plan = WholeFileTarget(destination: String(repeating: "a", count: 64),
            container: String(repeating: "b", count: 32), operation: String(repeating: "c", count: 32),
            node: String(repeating: "d", count: 32), base: 0, target: .firstMain(name: "file.bin"))
        let candidate = try files.prepare(id: plan.file, length: 0, check: {}, fill: { _, _ in
            XCTFail("empty body has no reads")
        }, validate: {})
        let save = try plan.record(candidate.identity)
        try intents.retain(save)
        XCTAssertThrowsError(try files.publish(candidate, check: { throw Stop.cancelled }))
        XCTAssertEqual(try intents.reopenForRecovery(operation: plan.operation).encode(), try save.encode())
        XCTAssertThrowsError(try files.reopen(candidate.identity, check: {}))
    }

    func testUncertainExtraLinkRefusesToOpenAndPreservesNames() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let files = WholeFileStage(directory: directory)
        let id = UUID()
        let identity = try files.freeze(id: id, length: 0, check: {}, fill: { _, _ in
            XCTFail("empty body has no reads")
        }, validate: {})
        let original = directory.appendingPathComponent(id.uuidString)
        let extra = directory.appendingPathComponent(".extra")
        try FileManager.default.linkItem(at: original, to: extra)
        XCTAssertThrowsError(try files.reopen(identity, check: {}))
        XCTAssertTrue(FileManager.default.fileExists(atPath: original.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: extra.path))
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
