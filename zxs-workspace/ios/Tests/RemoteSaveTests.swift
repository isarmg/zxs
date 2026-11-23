import Foundation
import XCTest
@testable import ZipPreview

final class RemoteSaveTests: XCTestCase {
    private let origin = "authenticated-shadow"
    private let container = String(repeating: "a", count: 32)
    private let operation = String(repeating: "b", count: 32)
    private let node = String(repeating: "c", count: 32)
    private let head = String(repeating: "d", count: 32)
    private let branch = String(repeating: "e", count: 32)
    private let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    private func frozen(_ target: RemoteSave.Target, base: UInt64, text: String = "abc") throws -> RemoteSave {
        try RemoteSave(destination: origin, container: container, operation: operation,
                       node: node, base: base, target: target, draft: LocalDraft.Revision(bytes: Data(text.utf8)))
    }

    private func receipt(target: String, sequence: String = "42") -> String {
        #"{"container_id":"\#(container)","operation_id":"\#(operation)","sequence":\#(sequence),"kind":"save_node","node_id":"\#(node)","length":3,"sha256":"\#(digest)","target":\#(target)}"#
    }

    func testFullWireShapesBindBodyTargetAndOriginalOperation() throws {
        let first = try frozen(.firstMain(name: "draft.txt"), base: 0)
        let append = try frozen(.append(branch: "main", head: head), base: 41)
        let fork = try frozen(.forkMain(branch: branch, from: head), base: 41)
        for (save, expectedBranch, target) in [
            (first, "main", #"{"kind":"first_main","name":"draft.txt"}"#),
            (append, "main", #"{"kind":"branch","branch":"main","expected_head":"\#(head)"}"#),
            (fork, branch, #"{"kind":"historical_main","branch":"\#(branch)","from_node":"\#(head)"}"#)
        ] {
            let post = try save.upload(for: origin)
            XCTAssertEqual(post.method, "POST")
            XCTAssertEqual(post.path, "/zxs/v1/containers/\(container)/branches/\(expectedBranch)/nodes")
            XCTAssertEqual(post.headers["Zxs-Operation"], operation)
            XCTAssertEqual(post.headers["Zxs-Node"], node)
            XCTAssertEqual(post.headers["Zxs-Sha256"], digest)
            XCTAssertEqual(post.body, Data("abc".utf8))
            XCTAssertEqual(try save.lookup(for: origin).path, "/zxs/v1/containers/\(container)/operations/\(operation)")
            XCTAssertEqual(try save.receipt(status: 200, bytes: Data(receipt(target: target).utf8),
                                            isLookup: true, authenticatedDestination: origin), .committed(42))
        }
        XCTAssertEqual(try first.upload(for: origin).headers["Zxs-Initial-Name"], "64726166742e747874")
        XCTAssertEqual(try append.upload(for: origin).headers["Zxs-Base-Node"], head)
        XCTAssertEqual(try fork.upload(for: origin).headers["Zxs-Fork-From-Main"], head)
        XCTAssertNil(try fork.upload(for: origin).headers["Zxs-Base-Node"])
    }

    func testUnmatchedAndUnknownEvidenceNeverConfirms() throws {
        let save = try frozen(.append(branch: "main", head: head), base: 41)
        let target = #"{"kind":"branch","branch":"main","expected_head":"\#(head)"}"#
        let exact = receipt(target: target)
        for changed in [
            exact.replacingOccurrences(of: operation, with: branch),
            exact.replacingOccurrences(of: digest, with: String(repeating: "a", count: 64)),
            exact.replacingOccurrences(of: head, with: node),
            exact.replacingOccurrences(of: #""sequence":42"#, with: #""sequence":42,"sequence":42"#),
            receipt(target: target, sequence: "41"),
            receipt(target: target, sequence: "18446744073709551616")
        ] {
            XCTAssertEqual(try save.receipt(status: 200, bytes: Data(changed.utf8),
                                            isLookup: true, authenticatedDestination: origin), .unknown)
        }
        let absent = Data(#"{"state":"not_found"}"#.utf8)
        XCTAssertEqual(try save.receipt(status: 404, bytes: absent, isLookup: true,
                                        authenticatedDestination: origin), .absent)
        XCTAssertEqual(try save.receipt(status: 404, bytes: absent, isLookup: false,
                                        authenticatedDestination: origin), .unknown)
        XCTAssertThrowsError(try save.upload(for: "different-origin"))
    }

    func testServerDisplayNameAndDistinctNodeRules() throws {
        let utf8 = "e\u{301}的.txt"
        let save = try frozen(.firstMain(name: utf8), base: 0)
        XCTAssertEqual(try save.upload(for: origin).headers["Zxs-Initial-Name"], "65cc81e79a842e747874")
        let exact = receipt(target: #"{"kind":"first_main","name":"\#(utf8)"}"#)
        let composed = receipt(target: #"{"kind":"first_main","name":"é的.txt"}"#)
        XCTAssertEqual(try save.receipt(status: 200, bytes: Data(exact.utf8),
                                        isLookup: true, authenticatedDestination: origin), .committed(42))
        XCTAssertEqual(try save.receipt(status: 200, bytes: Data(composed.utf8),
                                        isLookup: true, authenticatedDestination: origin), .unknown)
        _ = try frozen(.firstMain(name: String(repeating: "汉", count: 85)), base: 0)
        for name in ["", ".", "..", "x.", "x ", "x/y", "x\0", "x\u{85}", "x?", String(repeating: "汉", count: 86)] {
            XCTAssertThrowsError(try frozen(.firstMain(name: name), base: 0))
        }
        XCTAssertThrowsError(try frozen(.append(branch: "main", head: node), base: 41))
        XCTAssertThrowsError(try frozen(.forkMain(branch: branch, from: node), base: 41))
    }

    func testRetainedSameOperationReopensFrozenBytesAndRejectsReplacement() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RemoteSaveStore(directory: root)
        let saved = try frozen(.firstMain(name: "draft.txt"), base: 0)
        try store.retain(saved)
        try store.retain(saved) // exact replay only; no replacement
        let changed = try frozen(.firstMain(name: "draft.txt"), base: 0, text: "same operation, changed body")
        XCTAssertThrowsError(try store.retain(changed))
        let reopened = try RemoteSaveStore(directory: root).reopen(operation: operation)
        XCTAssertEqual(try reopened.upload(for: origin).body, Data("abc".utf8))

        let record = root.appendingPathComponent(operation + ".json")
        let diskBytes = try Data(contentsOf: record)
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: diskBytes) as? [String: String])
        let innerBase64 = try XCTUnwrap(envelope["record"])
        let inner = try XCTUnwrap(Data(base64Encoded: innerBase64))
        let rawIntention = try XCTUnwrap(String(data: inner, encoding: .utf8))
        let mismatched = rawIntention.replacingOccurrences(of: origin, with: "restored-other-origin")
        XCTAssertNotEqual(rawIntention, mismatched)
        let changedEnvelope = String(decoding: diskBytes, as: UTF8.self)
            .replacingOccurrences(of: innerBase64, with: Data(mismatched.utf8).base64EncodedString())
        try Data(changedEnvelope.utf8).write(to: record)
        XCTAssertThrowsError(try store.reopen(operation: operation))
    }
}
