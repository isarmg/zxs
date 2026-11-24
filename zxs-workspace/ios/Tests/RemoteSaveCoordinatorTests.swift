import Foundation
import XCTest
@testable import ZipPreview

private actor ScriptedSaveExchange: RemoteSaveExchange {
    nonisolated let destination: String
    nonisolated let acceptsFrozenFiles: Bool
    private var scripted: [RemoteSavePacket?]
    private let checkPOST: @Sendable () throws -> Void
    private(set) var methods: [String] = []
    private(set) var requests: [RemoteSave.Request] = []

    init(destination: String, scripted: [RemoteSavePacket?], acceptsFrozenFiles: Bool = false,
         checkPOST: @escaping @Sendable () throws -> Void = {}) {
        self.destination = destination
        self.scripted = scripted
        self.acceptsFrozenFiles = acceptsFrozenFiles
        self.checkPOST = checkPOST
    }

    func once(_ request: RemoteSave.Request) async throws -> RemoteSavePacket {
        methods.append(request.method)
        requests.append(request)
        if request.method == "POST" { try checkPOST() }
        guard !scripted.isEmpty, let response = scripted.removeFirst() else { throw TestFailure.interrupted }
        return response
    }

    enum TestFailure: Error { case interrupted }
}

private actor BlockingLookup: RemoteSaveExchange {
    nonisolated let destination: String
    private var blocked: CheckedContinuation<RemoteSavePacket, Never>?
    private var waiter: CheckedContinuation<Void, Never>?

    init(destination: String) { self.destination = destination }
    func once(_ request: RemoteSave.Request) async throws -> RemoteSavePacket {
        await withCheckedContinuation { continuation in
            blocked = continuation
            waiter?.resume()
            waiter = nil
        }
    }
    func waitForLookup() async {
        if blocked != nil { return }
        await withCheckedContinuation { waiter = $0 }
    }
    func finish(_ packet: RemoteSavePacket) { blocked?.resume(returning: packet); blocked = nil }
}

final class RemoteSaveCoordinatorTests: XCTestCase {
    private let destination = "fixture-authority-and-account"
    private let operation = String(repeating: "b", count: 32)
    private let container = String(repeating: "a", count: 32)
    private let node = String(repeating: "c", count: 32)
    private var absent: RemoteSavePacket { .init(status: 404, bytes: Data(#"{"state":"not_found"}"#.utf8)) }
    private var recorded: RemoteSavePacket {
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        let json = #"{"container_id":"\#(container)","operation_id":"\#(operation)","sequence":1,"kind":"save_node","node_id":"\#(node)","length":3,"sha256":"\#(digest)","target":{"kind":"first_main","name":"draft.txt"}}"#
        return .init(status: 200, bytes: Data(json.utf8))
    }

    func testLostReplyRetainsAttemptThenFindsOriginalWithoutResending() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = RemoteSaveStore(directory: directory)
        let save = try RemoteSave(destination: destination, container: container, operation: operation,
                                  node: node, base: 0, target: .firstMain(name: "draft.txt"),
                                  draft: LocalDraft.Revision(bytes: Data("abc".utf8)))
        try store.retain(save)
        let statusOnly = ScriptedSaveExchange(destination: destination, scripted: [absent])
        let start = RemoteSaveCoordinator(store: store)
        let before = try await start.run(consent: save.confirmation(), choice: .lookup, over: statusOnly)
        let beforeMethods = await statusOnly.methods
        XCTAssertEqual(before, .absent)
        XCTAssertEqual(beforeMethods, ["GET"])
        XCTAssertFalse(try store.attempted(save))

        let originalID = operation
        let failed = ScriptedSaveExchange(destination: destination, scripted: [absent, nil]) {
            let marker = directory.appendingPathComponent(originalID + ".attempt")
            XCTAssertEqual(try Data(contentsOf: marker), try save.encode())
        }
        do {
            _ = try await start.run(consent: save.confirmation(), choice: .submitFirst, over: failed)
            XCTFail("expected loss")
        } catch ScriptedSaveExchange.TestFailure.interrupted {}
        let firstMethods = await failed.methods
        XCTAssertEqual(firstMethods, ["GET", "POST"])
        XCTAssertTrue(try store.attempted(save))

        let reopened = RemoteSaveCoordinator(store: RemoteSaveStore(directory: directory))
        let absentAfterLoss = ScriptedSaveExchange(destination: destination, scripted: [absent])
        let rejectedRetry = try await reopened.run(consent: save.confirmation(), choice: .submitFirst, over: absentAfterLoss)
        let noRetryMethods = await absentAfterLoss.methods
        XCTAssertEqual(rejectedRetry, .requiresExplicitRetry)
        XCTAssertEqual(noRetryMethods, ["GET"])

        let found = ScriptedSaveExchange(destination: destination, scripted: [recorded])
        let resolved = try await reopened.run(consent: save.confirmation(), choice: .retrySameOperation, over: found)
        let foundMethods = await found.methods
        XCTAssertEqual(resolved, .committed(1))
        XCTAssertEqual(foundMethods, ["GET"])
        XCTAssertEqual(try store.cachedReceipt(save), recorded.bytes)

        let unused = ScriptedSaveExchange(destination: destination, scripted: [])
        let cached = try await reopened.run(consent: save.confirmation(), choice: .lookup, over: unused)
        let cachedMethods = await unused.methods
        XCTAssertEqual(cached, .committed(1))
        XCTAssertEqual(cachedMethods, [])

        let receiptPath = directory.appendingPathComponent(operation + ".receipt")
        let envelopeData = try Data(contentsOf: receiptPath)
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: envelopeData) as? [String: String])
        let exactValue = try XCTUnwrap(envelope["exact"])
        let changed = Data(String(decoding: recorded.bytes, as: UTF8.self)
            .replacingOccurrences(of: #""sequence":1"#, with: #""sequence":2"#).utf8)
        let tampered = String(decoding: envelopeData, as: UTF8.self)
            .replacingOccurrences(of: exactValue, with: changed.base64EncodedString())
        try Data(tampered.utf8).write(to: receiptPath)
        XCTAssertThrowsError(try store.cachedReceipt(save))
    }

    func testExplicitRetryUsesIdenticalBodyAndSameIDAndBlocksConcurrentAction() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = RemoteSaveStore(directory: directory)
        let save = try RemoteSave(destination: destination, container: container, operation: operation,
                                  node: node, base: 0, target: .firstMain(name: "draft.txt"),
                                  draft: LocalDraft.Revision(bytes: Data("abc".utf8)))
        try store.retain(save)
        let coordinator = RemoteSaveCoordinator(store: store)
        let gate = BlockingLookup(destination: destination)
        let waiting = Task { try await coordinator.run(consent: save.confirmation(), choice: .lookup, over: gate) }
        await gate.waitForLookup()
        let duplicate = ScriptedSaveExchange(destination: destination, scripted: [absent])
        do {
            _ = try await coordinator.run(consent: save.confirmation(), choice: .submitFirst, over: duplicate)
            XCTFail("expected busy")
        } catch RemoteSaveCoordinator.Failure.busy {}
        await gate.finish(absent)
        let lookup = try await waiting.value
        XCTAssertEqual(lookup, .absent)
        let neverSent = await duplicate.methods
        XCTAssertTrue(neverSent.isEmpty)

        let first = ScriptedSaveExchange(destination: destination, scripted: [absent, nil])
        do {
            _ = try await coordinator.run(consent: save.confirmation(), choice: .submitFirst, over: first)
            XCTFail("expected lost reply")
        } catch ScriptedSaveExchange.TestFailure.interrupted {}
        let retry = ScriptedSaveExchange(destination: destination, scripted: [absent, recorded])
        let result = try await coordinator.run(consent: save.confirmation(), choice: .retrySameOperation, over: retry)
        XCTAssertEqual(result, .committed(1))
        let firstRequests = await first.requests, retriedRequests = await retry.requests
        XCTAssertEqual(firstRequests.map(\.method), ["GET", "POST"])
        XCTAssertEqual(retriedRequests.map(\.method), ["GET", "POST"])
        XCTAssertEqual(firstRequests.last?.path, retriedRequests.last?.path)
        XCTAssertEqual(firstRequests.last?.headers, retriedRequests.last?.headers)
        XCTAssertEqual(firstRequests.last?.body, retriedRequests.last?.body)
    }

    func testDamagedPrimaryAfterLostReplyPermitsOnlyReceiptLookup() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = RemoteSaveStore(directory: directory)
        let save = try RemoteSave(destination: destination, container: container, operation: operation,
                                  node: node, base: 0, target: .firstMain(name: "draft.txt"),
                                  draft: LocalDraft.Revision(bytes: Data("abc".utf8)))
        try store.retain(save)
        let first = ScriptedSaveExchange(destination: destination, scripted: [absent, nil])
        do {
            _ = try await RemoteSaveCoordinator(store: store).run(consent: save.confirmation(), choice: .submitFirst, over: first)
            XCTFail("expected lost reply")
        } catch ScriptedSaveExchange.TestFailure.interrupted {}

        let original = directory.appendingPathComponent(operation + ".json")
        for removed in [false, true] {
            if removed { try FileManager.default.removeItem(at: original) }
            else { try Data("damaged".utf8).write(to: original) }
            let reloaded = RemoteSaveCoordinator(store: RemoteSaveStore(directory: directory))
            let retry = ScriptedSaveExchange(destination: destination, scripted: [absent])
            do {
                _ = try await reloaded.run(consent: save.confirmation(), choice: .retrySameOperation, over: retry)
                XCTFail("missing exact primary must prevent retry")
            } catch RemoteSave.Failure.invalidStoredRecord {}
            let methods = await retry.methods
            XCTAssertEqual(methods, ["GET"])
            if removed { XCTAssertFalse(FileManager.default.fileExists(atPath: original.path)) }
            else { XCTAssertEqual(try Data(contentsOf: original), Data("damaged".utf8)) }
        }

        let resolved = ScriptedSaveExchange(destination: destination, scripted: [recorded])
        let found = try await RemoteSaveCoordinator(store: store)
            .run(consent: save.confirmation(), choice: .lookup, over: resolved)
        XCTAssertEqual(found, .committed(1))
        let methods = await resolved.methods
        XCTAssertEqual(methods, ["GET"])
    }

    func testChangedFrozenSelectionCannotStartEvenLookup() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = RemoteSaveStore(directory: directory)
        let confirmed = try RemoteSave(destination: destination, container: container, operation: operation,
            node: node, base: 0, target: .firstMain(name: "old.txt"),
            draft: LocalDraft.Revision(bytes: Data("abc".utf8)))
        try store.retain(confirmed)
        let consent = try confirmed.confirmation()
        let changed = try RemoteSave(destination: destination, container: container, operation: operation,
            node: node, base: 0, target: .firstMain(name: "new.txt"),
            draft: LocalDraft.Revision(bytes: Data("other".utf8)))
        try Data(changed.encode()).write(to: directory.appendingPathComponent(operation + ".json"))
        let unused = ScriptedSaveExchange(destination: destination, scripted: [absent])
        do {
            _ = try await RemoteSaveCoordinator(store: store).run(consent: consent, choice: .lookup, over: unused)
            XCTFail("changed selector must not go on the wire")
        } catch RemoteSaveCoordinator.Failure.selectionChanged {}
        let network = await unused.methods
        XCTAssertTrue(network.isEmpty)
    }

    func testActiveHeadlessBasicLoginAndColonPassword() throws {
        let credential = try MobileSaveCredential(username: "alice_1", password: "a:b:中文")
        let payload = String(credential.basic().dropFirst(6))
        let decoded = try XCTUnwrap(Data(base64Encoded: payload))
        XCTAssertEqual(decoded, Data("alice_1:a:b:中文".utf8))
        XCTAssertThrowsError(try MobileSaveCredential(username: "alice:1", password: "a:b"))
        XCTAssertThrowsError(try MobileSaveCredential(username: "Alice", password: "a:b"))
        XCTAssertThrowsError(try MobileSaveCredential(username: "1", password: "a\nb"))
    }

    func testDiskBackedRecordNeverCarriesInlineBodyAndCanQueryWithMissingBody() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let stage = WholeFileStage(directory: directory.appendingPathComponent("files"))
        let store = RemoteSaveStore(directory: directory.appendingPathComponent("operations"))
        let id = UUID()
        let staged = try stage.freeze(id: id, length: 3, check: {}, fill: { _, buffer in
            _ = Data("abc".utf8).copyBytes(to: buffer)
        }, validate: {})
        let save = try RemoteSave(destination: destination, container: container, operation: operation,
            node: node, base: 0, target: .firstMain(name: "draft.txt"), wholeFile: staged)
        try store.retain(save)
        XCTAssertEqual(try store.reopen(operation: operation).diskBody, staged)
        XCTAssertLessThan(try save.encode().count, 2_048)
        XCTAssertThrowsError(try save.upload(for: destination))
        let coordinator = RemoteSaveCoordinator(store: store, wholeFiles: stage)
        let unsupported = ScriptedSaveExchange(destination: destination, scripted: [absent])
        do {
            _ = try await coordinator.run(consent: save.confirmation(), choice: .submitFirst, over: unsupported)
            XCTFail("must not fall back to inline transport")
        } catch RemoteSave.Failure.invalidIntent {}
        let unsupportedMethods = await unsupported.methods
        XCTAssertEqual(unsupportedMethods, ["GET"])
        XCTAssertFalse(try store.attempted(save))

        let loss = ScriptedSaveExchange(destination: destination, scripted: [absent, nil], acceptsFrozenFiles: true)
        do {
            _ = try await coordinator.run(consent: save.confirmation(), choice: .submitFirst, over: loss)
            XCTFail("expected interrupted upload")
        } catch ScriptedSaveExchange.TestFailure.interrupted {}
        let request = await loss.requests.last
        XCTAssertNil(request?.body)
        XCTAssertEqual(request?.headers["Content-Length"], "3")
        XCTAssertEqual(try request?.frozenFile?.readChunk(maximum: 65_536), Data("abc".utf8))
        XCTAssertTrue(try store.attempted(save))
        try FileManager.default.removeItem(at: directory.appendingPathComponent("files").appendingPathComponent(id.uuidString))

        let cold = RemoteSaveCoordinator(store: store, wholeFiles: stage)
        let noBody = ScriptedSaveExchange(destination: destination, scripted: [absent], acceptsFrozenFiles: true)
        do {
            _ = try await cold.run(consent: save.confirmation(), choice: .retrySameOperation, over: noBody)
            XCTFail("missing protected body must suppress POST")
        } catch WholeFileStage.Failure.invalidFile {}
        let missingMethods = await noBody.methods
        XCTAssertEqual(missingMethods, ["GET"])
        try FileManager.default.removeItem(at: directory.appendingPathComponent("operations").appendingPathComponent(operation + ".json"))
        let found = ScriptedSaveExchange(destination: destination, scripted: [recorded])
        let result = try await cold.run(consent: save.confirmation(), choice: .lookup, over: found)
        let foundMethods = await found.methods
        XCTAssertEqual(result, .committed(1))
        XCTAssertEqual(foundMethods, ["GET"])
    }
}
