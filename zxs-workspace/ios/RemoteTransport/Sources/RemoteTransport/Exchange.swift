import CryptoKit
import Foundation
import NIOCore
import NIOHPACK
import NIOHTTP2

public struct R3Request: Sendable {
    let method: String
    let path: String
    let headers: [String: String]
    let body: Data?

    public init(method: String, path: String, headers: [String: String], body: Data?) {
        self.method = method
        self.path = path
        self.headers = headers
        self.body = body
    }

    func frames(authority: String, authorization: () throws -> String) throws -> [HTTP2Frame.FramePayload] {
        try validateRequest()
        let credential = try authorization()
        guard (1...8192).contains(credential.utf8.count), Self.headerText(credential) else {
            throw TransportFailure.invalidRequest
        }
        var fields = HPACKHeaders([
            (":method", method), (":scheme", "https"), (":authority", authority), (":path", path),
            ("accept", "application/json"), ("accept-encoding", "identity"),
        ])
        fields.add(name: "authorization", value: credential, indexing: .neverIndexed)
        for (name, value) in headers.sorted(by: { $0.key < $1.key }) {
            fields.add(name: name.lowercased(), value: value)
        }

        let empty = body?.isEmpty ?? true
        let head = HTTP2Frame.FramePayload.headers(.init(headers: fields, endStream: empty))
        guard !empty, let body else { return [head] }
        var buffer = ByteBufferAllocator().buffer(capacity: body.count)
        buffer.writeBytes(body)
        return [head, .data(.init(data: .byteBuffer(buffer), endStream: true))]
    }

    private func validateRequest() throws {
        let fields = path.split(separator: "/", omittingEmptySubsequences: false)
        guard fields.count >= 7, fields[0] == "", fields[1] == "zxs", fields[2] == "v1",
              fields[3] == "containers", Self.validID(fields[4]) else { throw TransportFailure.invalidRequest }

        if method == "GET" {
            guard fields.count == 7, fields[5] == "operations", Self.validID(fields[6]),
                  headers.isEmpty, body == nil else { throw TransportFailure.invalidRequest }
            return
        }
        guard method == "POST", fields.count == 8, fields[5] == "branches", fields[7] == "nodes",
              fields[6] == "main" || Self.validID(fields[6]),
              let body, body.count <= 256 * 1024 else { throw TransportFailure.invalidRequest }

        let required: Set<String> = ["content-type", "content-length", "zxs-operation", "zxs-node", "zxs-sha256"]
        let optional: Set<String> = ["zxs-base-node", "zxs-initial-name", "zxs-fork-from-main"]
        var normalized: [String: String] = [:]
        for (key, value) in headers {
            let name = key.lowercased()
            guard key.utf8.allSatisfy({ $0 < 128 }), required.contains(name) || optional.contains(name),
                  normalized.updateValue(value, forKey: name) == nil, Self.headerText(value)
            else { throw TransportFailure.invalidRequest }
        }
        let digest = SHA256.hash(data: body).map { String(format: "%02x", $0) }.joined()
        guard normalized["content-type"] == "application/octet-stream",
              normalized["content-length"] == String(body.count), normalized["zxs-sha256"] == digest,
              let operation = normalized["zxs-operation"], Self.validID(operation[...]),
              let node = normalized["zxs-node"], Self.validID(node[...]) else { throw TransportFailure.invalidRequest }
        let from = normalized["zxs-fork-from-main"]
        let base = normalized["zxs-base-node"]
        let name = normalized["zxs-initial-name"]
        switch (from, base, name) {
        case let (source?, nil, nil):
            guard fields[6] != "main", Self.validID(source[...]), source != node else { throw TransportFailure.invalidRequest }
        case let (nil, head?, nil):
            guard Self.validID(head[...]), head != node else { throw TransportFailure.invalidRequest }
        case let (nil, displayBase?, display?):
            guard displayBase == "0", fields[6] == "main", (2...510).contains(display.utf8.count), display.utf8.count % 2 == 0,
                  Self.hex(display.utf8) else { throw TransportFailure.invalidRequest }
        default: throw TransportFailure.invalidRequest
        }
    }

    private static func validID(_ id: Substring) -> Bool {
        id.utf8.count == 32 && hex(id.utf8) && id.utf8.contains { $0 != 48 }
    }
    private static func hex<T: Sequence>(_ bytes: T) -> Bool where T.Element == UInt8 {
        bytes.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    private static func headerText(_ value: String) -> Bool {
        !value.isEmpty && value.utf8.allSatisfy { (32...126).contains($0) }
    }
}

public struct R3Response: Equatable, Sendable {
    public let status: Int
    public let bytes: Data
}

struct R3ResponseAccumulator {
    private var status: Int?
    private var size = 0
    private var bytes = Data()
    private var ended = false

    mutating func head(_ fields: HPACKHeaders, end: Bool) throws -> R3Response? {
        guard status == nil, !ended else { throw TransportFailure.invalidResponse } // no trailers
        var all: [String: [String]] = [:]
        for field in fields {
            guard field.name != "", field.name.utf8.allSatisfy({ $0 < 128 }),
                  field.name == field.name.lowercased(),
                  !field.name.hasPrefix(":") || field.name == ":status" else {
                throw TransportFailure.invalidResponse
            }
            all[field.name, default: []].append(field.value)
        }
        guard !["transfer-encoding", "content-encoding", "content-range", "trailer"].contains(where: {
            all[$0] != nil
        }), let statuses = all[":status"], statuses.count == 1, let value = Int(statuses[0]),
              String(value) == statuses[0], (200...599).contains(value),
              let lengths = all["content-length"], lengths.count == 1,
              lengths[0].utf8.count <= 4, let length = Int(lengths[0]), String(length) == lengths[0],
              (0...2048).contains(length) else { throw TransportFailure.invalidResponse }
        if value == 200 || value == 404 {
            guard all["content-type"] == ["application/json"] else { throw TransportFailure.invalidResponse }
        }
        self.status = value
        self.size = length
        return try complete(end)
    }

    mutating func data(_ block: ByteBuffer, end: Bool) throws -> R3Response? {
        guard status != nil, !ended, block.readableBytes <= size - bytes.count else {
            throw TransportFailure.invalidResponse
        }
        bytes.append(contentsOf: block.readableBytesView)
        return try complete(end)
    }

    private mutating func complete(_ end: Bool) throws -> R3Response? {
        guard end else { return nil }
        guard let status, bytes.count == size else { throw TransportFailure.invalidResponse }
        ended = true
        return R3Response(status: status, bytes: bytes)
    }
}

final class R3ResponseReceiver: ChannelInboundHandler {
    typealias InboundIn = HTTP2Frame.FramePayload

    private let result: EventLoopPromise<R3Response>
    private var pending = R3ResponseAccumulator()
    private(set) var finished = false

    init(result: EventLoopPromise<R3Response>) { self.result = result }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        guard !finished else { return }
        do {
            let resolved: R3Response?
            switch unwrapInboundIn(data) {
            case let .headers(head): resolved = try pending.head(head.headers, end: head.endStream)
            case let .data(payload):
                guard case let .byteBuffer(block) = payload.data else { throw TransportFailure.invalidResponse }
                resolved = try pending.data(block, end: payload.endStream)
            case .rstStream: throw TransportFailure.interrupted
            default: return // connection/stream framing remains NIO's responsibility
            }
            if let resolved {
                finished = true
                result.succeed(resolved)
            }
        } catch { fail(error) }
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) { fail(error) }
    func channelInactive(context: ChannelHandlerContext) {
        fail(TransportFailure.interrupted)
        context.fireChannelInactive()
    }

    func fail(_ error: Error) {
        guard !finished else { return }
        finished = true
        result.fail(error)
    }
}
