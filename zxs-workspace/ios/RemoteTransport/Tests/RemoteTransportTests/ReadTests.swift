import Foundation
import NIOCore
import NIOHPACK
import XCTest
@testable import RemoteTransport

final class ReadTests: XCTestCase {
    private let container = String(repeating: "a", count: 32)
    private let node = String(repeating: "b", count: 32)
    private let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    func testExactReadRoutesAndOptionalCursors() throws {
        XCTAssertEqual(try R3Request.catalog(after: node).headers["zxs-after"], node)
        XCTAssertThrowsError(try R3Request.detail(container: container, sequence: nil, after: "main"))
        XCTAssertEqual(try R3Request.detail(container: container, sequence: 12, after: "main")
            .headers["zxs-detail-sequence"], "12")
        XCTAssertThrowsError(try R3Request.history(container: container, branch: "main", head: node, next: node))
        XCTAssertEqual(try R3Request.textNode(container: container, node: node, length: 3, sha256: digest).path,
            "/zxs/v1/containers/\(container)/nodes/\(node)/data")
        XCTAssertThrowsError(try R3Request.textNode(container: container, node: node, length: 256 * 1024 + 1, sha256: digest))
    }

    func testNodeNeedsExactMetadataHeaderAndBodyDigest() throws {
        let fields = HPACKHeaders([(":status", "200"), ("content-type", "application/octet-stream"),
                                   ("content-length", "3"), ("zxs-sha256", digest)])
        var body = ByteBufferAllocator().buffer(capacity: 3)
        body.writeString("abc")
        var exact = R3ResponseAccumulator(mode: .node(length: 3, digest: digest))
        XCTAssertNil(try exact.head(fields, end: false))
        XCTAssertEqual(try exact.data(body, end: true)?.bytes, Data("abc".utf8))

        var corrupt = R3ResponseAccumulator(mode: .node(length: 3, digest: digest))
        XCTAssertNil(try corrupt.head(fields, end: false))
        var different = ByteBufferAllocator().buffer(capacity: 3)
        different.writeString("xyz")
        XCTAssertThrowsError(try corrupt.data(different, end: true))

        var changedLength = R3ResponseAccumulator(mode: .node(length: 2, digest: digest))
        XCTAssertThrowsError(try changedLength.head(fields, end: false))
    }
}
