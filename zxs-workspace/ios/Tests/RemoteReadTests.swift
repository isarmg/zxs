import Foundation
import XCTest
@testable import ZipPreview

final class RemoteReadTests: XCTestCase {
    private let container = String(repeating: "a", count: 32)
    private let head = String(repeating: "b", count: 32)
    private let old = String(repeating: "c", count: 32)
    private let request = String(repeating: "d", count: 32)
    private let newNode = String(repeating: "e", count: 32)
    private let newBranch = String(repeating: "f", count: 32)
    private let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    private func wireNode(_ id: String, parent: String, sequence: UInt64) -> String {
        #"{"node_id":"\#(id)","operation_id":"\#(request)","parent_node":\#(parent),"original_branch":"main","sequence":\#(sequence),"length":3,"sha256":"\#(digest)"}"#
    }

    func testStrictSelectionAndExactAppendOrHistoricalFork() throws {
        let catalog = #"{"items":[{"container_id":"\#(container)","sequence":5,"name":"报告.txt"}],"next_after":null}"#
        let page = try RemoteRead.Catalog.select(Data(catalog.utf8), after: nil)
        let selected = try XCTUnwrap(page.items.first)
        let detail = #"{"container_id":"\#(container)","sequence":5,"name":"报告.txt","tags":[],"heads":[{"branch":"main","head":"\#(head)"}],"next_after":null}"#
        let branch = try XCTUnwrap(RemoteRead.Detail.select(Data(detail.utf8), expected: selected, after: nil).heads.first)
        let body = "[" + wireNode(head, parent: "\"" + old + "\"", sequence: 5)
            + "," + wireNode(old, parent: "null", sequence: 1) + "]"
        let history = #"{"head":"\#(head)","nodes":\#(body),"next":null}"#
        let decoded = try RemoteRead.History.select(Data(history.utf8), head: head, cursor: nil, upper: 5)
        XCTAssertEqual(decoded.nodes.count, 2)
        let append = try RemoteRead.TextSelection(container: selected, branch: branch, node: decoded.nodes[0],
                                                  original: Data("abc".utf8))
        let fork = try RemoteRead.TextSelection(container: selected, branch: branch, node: decoded.nodes[1],
                                                original: Data("abc".utf8))
        XCTAssertTrue(append.displayedExactly("abc"))
        XCTAssertFalse(append.displayedExactly("abc\n"))
        XCTAssertFalse(append.displayedExactly("abc\0"))
        for selection in [append, fork] {
            XCTAssertThrowsError(try selection.freeze(text: "abc", destination: "test",
                operationID: request, newNode: newNode, newBranch: newBranch))
        }
        let added = try append.freeze(text: "全文", destination: "test", operationID: request,
                                      newNode: newNode, newBranch: newBranch).upload(for: "test")
        XCTAssertEqual(added.path, "/zxs/v1/containers/\(container)/branches/main/nodes")
        XCTAssertEqual(added.headers["Zxs-Base-Node"], head)
        XCTAssertEqual(added.body, Data("全文".utf8))
        let branched = try fork.freeze(text: "更改", destination: "test", operationID: request,
                                      newNode: newNode, newBranch: newBranch).upload(for: "test")
        XCTAssertEqual(branched.headers["Zxs-Fork-From-Main"], old)
        XCTAssertNil(branched.headers["Zxs-Base-Node"])
        XCTAssertEqual(branched.path, "/zxs/v1/containers/\(container)/branches/\(newBranch)/nodes")
        XCTAssertThrowsError(try RemoteRead.TextSelection(container: selected, branch: branch,
                                                           node: decoded.nodes[0], original: Data("xyz".utf8)))
    }

    func testDuplicateFieldsStaleRevisionAndUnexpectedHistoryRejected() throws {
        let catalog = #"{"items":[{"container_id":"\#(container)","sequence":5,"name":"a.txt"}],"next_after":null}"#
        let item = try XCTUnwrap(RemoteRead.Catalog.select(Data(catalog.utf8), after: nil).items.first)
        XCTAssertThrowsError(try RemoteRead.Catalog.select(Data(catalog.utf8), after: container))
        let duplicate = catalog.replacingOccurrences(of: #""sequence":5"#, with: #""sequence":5,"sequence":5"#)
        XCTAssertThrowsError(try RemoteRead.Catalog.select(Data(duplicate.utf8), after: nil))
        let stale = #"{"container_id":"\#(container)","sequence":6,"name":"a.txt","tags":[],"heads":[],"next_after":null}"#
        XCTAssertThrowsError(try RemoteRead.Detail.select(Data(stale.utf8), expected: item, after: nil))
        let wrongNode = wireNode(old, parent: "null", sequence: 1)
        let jumped = #"{"head":"\#(head)","nodes":[\#(wrongNode)],"next":null}"#
        XCTAssertThrowsError(try RemoteRead.History.select(Data(jumped.utf8), head: head, cursor: nil, upper: 5))
    }
}
