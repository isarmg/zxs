import CryptoKit
import Foundation
import XCTest
@testable import ZipPreview

final class WholeFileStageTests: XCTestCase {
    private enum Stop: Error { case cancelled, sourceChanged }

    private func version(_ bytes: Data) -> RemoteRead.History.Node {
        RemoteRead.History.Node(nodeId: String(repeating: "1", count: 32),
            operationId: String(repeating: "2", count: 32), parentNode: nil, originalBranch: "main",
            sequence: 1, length: UInt64(bytes.count),
            sha256: SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined())
    }

    private func plan(_ target: WholeFileTarget.Selection, base: UInt64) -> WholeFileTarget {
        WholeFileTarget(destination: "fixture", container: String(repeating: "b", count: 32),
            operation: UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: ""),
            node: String(repeating: "d", count: 32), base: base, target: target)
    }

    // Same preparation entry point and storage as the picker. Substitute only
    // the coordinated original document's bounded reads with exact test bytes.
    private func stage(_ bytes: Data, for plan: WholeFileTarget, files: WholeFileStage,
                       drafts: RemoteSaveStore) -> WholeFileTarget.Preparation {
        plan.prepare(in: drafts) { beforePublish in
            let staged = try files.prepare(id: plan.file, length: bytes.count, check: {}, fill: { offset, slice in
                _ = bytes.copyBytes(to: slice, from: offset..<(offset + slice.count))
            }, validate: {})
            try beforePublish(staged.identity)
            return try files.publish(staged, check: {})
        }
    }

    func testUnchangedPickedFilesPublishNeitherIntentNorBody() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let filesURL = root.appendingPathComponent("files")
        let files = WholeFileStage(directory: filesURL)
        let drafts = RemoteSaveStore(directory: root.appendingPathComponent("intents"))
        let named = String(repeating: "c", count: 32)
        for bytes in [Data(), Data(repeating: 0xff, count: 65_537)] {
            let original = version(bytes)
            let choices: [WholeFileTarget.Selection] = [.append(branch: "main", original: original),
                .append(branch: named, original: original), .forkMain(branch: named, original: original)]
            for choice in choices {
                let request = plan(choice, base: 5)
                guard case .unchanged = stage(bytes, for: request, files: files, drafts: drafts)
                else { return XCTFail("a byte-identical update must not become sendable or a recovery entry") }
                XCTAssertEqual(try drafts.operationIDs(), [])
                XCTAssertFalse(FileManager.default.fileExists(atPath: filesURL.appendingPathComponent(request.file.uuidString).path))
                XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: filesURL.path), [])
            }
        }
    }

    func testFirstSaveAndChangedReplacementsStillRetainTheExactOperation() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let files = WholeFileStage(directory: root.appendingPathComponent("files"))
        let drafts = RemoteSaveStore(directory: root.appendingPathComponent("intents"))
        let oldBytes = Data([0xff, 0x00]), changed = Data([0xff, 0x01])
        let original = version(oldBytes)
        let branch = String(repeating: "c", count: 32)
        let cases: [(WholeFileTarget, RemoteSave.Target, Data)] = [
            (plan(.append(branch: branch, original: original), base: 5), .append(branch: branch, head: original.nodeId), changed),
            (plan(.forkMain(branch: branch, original: original), base: 5), .forkMain(branch: branch, from: original.nodeId), Data()),
            (plan(.firstMain(name: "first.bin"), base: 0), .firstMain(name: "first.bin"), oldBytes),
            (plan(.firstMain(name: "empty.bin"), base: 0), .firstMain(name: "empty.bin"), Data()),
        ]
        for (request, target, bytes) in cases {
            guard case let .retained(saved, true) = stage(bytes, for: request, files: files, drafts: drafts)
            else { return XCTFail("first saves and changed replacements must still be prepared") }
            let body = try XCTUnwrap(saved.diskBody)
            let expected = try RemoteSave(destination: request.destination, container: request.container,
                operation: request.operation, node: request.node, base: request.base, target: target, wholeFile: body)
            XCTAssertEqual(try drafts.reopenForRecovery(operation: request.operation).encode(), try expected.encode())
            let opened = try files.reopen(body, check: {})
            defer { try? opened.close() }
            XCTAssertEqual(try opened.readToEnd() ?? Data(), bytes)
        }
    }

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
