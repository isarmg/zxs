import Foundation
import NIOCore
import NIOHPACK
import NIOHTTP2
import XCTest
@testable import RemoteTransport

final class ExchangeTests: XCTestCase {
    private let container = String(repeating: "a", count: 32)
    private let operation = String(repeating: "b", count: 32)
    private let node = String(repeating: "c", count: 32)
    private var headers: [String: String] {
        ["Content-Type": "application/octet-stream", "Content-Length": "3",
         "Zxs-Operation": operation, "Zxs-Node": node,
         "Zxs-Sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
         "Zxs-Base-Node": "0", "Zxs-Initial-Name": "61"]
    }

    func testExactPostDataAndNeverIndexedCredential() throws {
        let address = try TransportAddress(host: "::1", port: 9443)
        let path = "/zxs/v1/containers/\(container)/branches/main/nodes"
        let built = try R3Request(method: "POST", path: path,
                                  headers: headers, body: Data("abc".utf8)).frames(authority: address.authority) {
            "Basic syntheticFixture"
        }
        XCTAssertEqual(built.count, 2)
        guard case let .headers(head) = built[0], case let .data(body) = built[1],
              case let .byteBuffer(buffer) = body.data else { return XCTFail("unexpected frames") }
        XCTAssertFalse(head.endStream)
        XCTAssertEqual(head.headers[":authority"], ["[::1]:9443"])
        XCTAssertEqual(head.headers[":path"], [path])
        XCTAssertEqual(head.headers.first(where: { $0.name == "authorization" })?.indexable, .neverIndexed)
        XCTAssertTrue(body.endStream)
        XCTAssertEqual(Array(buffer.readableBytesView), Array("abc".utf8))
    }

    func testInvalidPostCannotResolveCredential() throws {
        var conflicting = headers
        conflicting["content-length"] = "4"
        let path = "/zxs/v1/containers/\(container)/branches/main/nodes"
        for request in [
            R3Request(method: "POST", path: path, headers: conflicting, body: Data("abc".utf8)),
            R3Request(method: "POST", path: path, headers: headers, body: Data("xyz".utf8)),
            R3Request(method: "GET", path: path, headers: [:], body: nil),
            R3Request(method: "GET", path: "//evil.example", headers: [:], body: nil)
        ] {
            XCTAssertThrowsError(try request.frames(authority: "store.example:443") {
                XCTFail("credentials resolved for a malformed request")
                return "synthetic"
            })
        }
    }

    private func head(lengths: [String], encoding: String? = nil) -> HPACKHeaders {
        var fields = HPACKHeaders([(":status", "200"), ("content-type", "application/json")])
        for length in lengths { fields.add(name: "content-length", value: length) }
        if let encoding { fields.add(name: "content-encoding", value: encoding) }
        return fields
    }

    func testResponseRequiresUniqueLengthsExactBytesAndEnd() throws {
        var valid = R3ResponseAccumulator()
        XCTAssertNil(try valid.head(head(lengths: ["2"]), end: false))
        var body = ByteBufferAllocator().buffer(capacity: 2)
        body.writeString("{}")
        XCTAssertEqual(try valid.data(body, end: true), R3Response(status: 200, bytes: Data("{}".utf8)))

        var duplicate = R3ResponseAccumulator()
        XCTAssertThrowsError(try duplicate.head(head(lengths: ["2", "2"]), end: false))
        var transformed = R3ResponseAccumulator()
        XCTAssertThrowsError(try transformed.head(head(lengths: ["2"], encoding: "gzip"), end: false))
        var short = R3ResponseAccumulator()
        XCTAssertNil(try short.head(head(lengths: ["3"]), end: false))
        XCTAssertThrowsError(try short.data(body, end: true))
        var trailers = R3ResponseAccumulator()
        XCTAssertNil(try trailers.head(head(lengths: ["2"]), end: false))
        XCTAssertThrowsError(try trailers.head(HPACKHeaders([("etag", "extra")]), end: true))
    }
}
