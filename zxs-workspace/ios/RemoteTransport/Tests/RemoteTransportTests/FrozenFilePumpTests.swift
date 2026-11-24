import Foundation
import NIOCore
import NIOEmbedded
import NIOHPACK
import NIOHTTP2
import XCTest
@testable import RemoteTransport

final class FrozenFilePumpTests: XCTestCase {
    private let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    func testDemandAndActualEndWithMatchingDigest() throws {
        var chunks = VerifiedFileChunks(length: 3, sha256: digest)
        XCTAssertEqual(chunks.demand, 3)
        guard case let .bytes(first) = try chunks.accept(Data("a".utf8))
        else { return XCTFail("missing chunk") }
        XCTAssertEqual(first, Data("a".utf8))
        XCTAssertEqual(chunks.demand, 2)
        guard case .bytes = try chunks.accept(Data("bc".utf8)) else { return XCTFail("missing chunk") }
        XCTAssertEqual(chunks.demand, 1)
        guard case .checkedEnd = try chunks.accept(nil) else { return XCTFail("lost verifying EOF") }
        XCTAssertThrowsError(try chunks.accept(nil))
    }

    func testShortOversizedCorruptOrExtendedBodyCannotSeal() throws {
        var short = VerifiedFileChunks(length: 3, sha256: digest)
        XCTAssertThrowsError(try short.accept(nil))
        var larger = VerifiedFileChunks(length: 3, sha256: digest)
        XCTAssertThrowsError(try larger.accept(Data("abcd".utf8)))
        var corrupt = VerifiedFileChunks(length: 3, sha256: digest)
        _ = try corrupt.accept(Data("xyz".utf8))
        XCTAssertThrowsError(try corrupt.accept(nil))
        var extra = VerifiedFileChunks(length: 3, sha256: digest)
        _ = try extra.accept(Data("abc".utf8))
        XCTAssertThrowsError(try extra.accept(Data("d".utf8)))
        XCTAssertEqual(VerifiedFileChunks(length: .max, sha256: digest).demand, 16_384)
    }

    func testDiskHeadersNeverEagerlyReadOrEndStream() throws {
        let id = String(repeating: "a", count: 32)
        let operation = String(repeating: "b", count: 32)
        let node = String(repeating: "c", count: 32)
        let source = R3UploadSource(length: 400_000, sha256: digest, read: { _ in
            XCTFail("preparing headers cannot read the file")
            return nil
        })
        let request = R3Request.stagedSave(path: "/zxs/v1/containers/\(id)/branches/main/nodes",
            headers: ["Content-Type": "application/octet-stream", "Content-Length": "400000",
                      "Zxs-Operation": operation, "Zxs-Node": node, "Zxs-Sha256": digest,
                      "Zxs-Base-Node": "0", "Zxs-Initial-Name": "61"], source: source)
        let frames = try request.frames(authority: "fixture.example:443") { "Basic fixture" }
        XCTAssertEqual(frames.count, 1)
        guard case let .headers(head) = frames[0] else { return XCTFail("expected headers only") }
        XCTAssertEqual(head.headers["content-length"], ["400000"])
        XCTAssertFalse(head.endStream)
    }

    func testExactStreamHandlerRejectsEarlySuccessAtHeadersAndAcceptsEarlyConflict() throws {
        func exchange(code: String, written: Bool, ended: Bool, completeLater: Bool) throws -> Result<R3Response, Error>? {
            let channel = EmbeddedChannel()
            defer { _ = try? channel.finish() }
            let promise = channel.eventLoop.makePromise(of: R3Response.self)
            let proof = FileSendProof()
            if written { proof.endWasWritten() }
            let handler = R3ResponseReceiver(result: promise, mode: .receipt, fileProof: proof)
            try channel.pipeline.syncOperations.addHandler(handler)
            var observed: Result<R3Response, Error>?
            promise.futureResult.whenComplete { observed = $0 }
            let fields = HPACKHeaders([(":status", code), ("content-length", "0"),
                                       ("content-type", "application/json")])
            _ = try channel.writeInbound(HTTP2Frame.FramePayload.headers(.init(headers: fields, endStream: ended)))
            if completeLater {
                proof.endWasWritten()
                let empty = ByteBufferAllocator().buffer(capacity: 0)
                _ = try channel.writeInbound(HTTP2Frame.FramePayload.data(.init(data: .byteBuffer(empty), endStream: true)))
            }
            channel.embeddedEventLoop.run()
            return observed
        }
        guard case .failure? = try exchange(code: "200", written: false, ended: false, completeLater: true)
        else { return XCTFail("even a later verified write cannot rescue early success HEADERS") }
        guard case let .success(conflict)? = try exchange(code: "409", written: false, ended: true, completeLater: false)
        else { return XCTFail("an early conflict should be reported") }
        XCTAssertEqual(conflict.status, 409)
        guard case let .success(saved)? = try exchange(code: "200", written: true, ended: true, completeLater: false)
        else { return XCTFail("completed request should admit exact response framing") }
        XCTAssertEqual(saved.status, 200)
    }
}
