import CryptoKit
import Foundation
import NIOCore
import NIOHPACK
import NIOHTTP2

public struct R3UploadSource: Sendable {
    public let length: Int
    public let sha256: String
    let read: @Sendable (Int) throws -> Data?

    public init(length: Int, sha256: String, read: @escaping @Sendable (Int) throws -> Data?) {
        self.length = length; self.sha256 = sha256; self.read = read
    }
}

public struct R3Request: Sendable {
    let method: String
    let path: String
    let headers: [String: String]
    let body: Data?
    let disk: R3UploadSource?
    enum Mode: Sendable { case receipt, metadata, node(length: Int, digest: String) }
    let mode: Mode

    public init(method: String, path: String, headers: [String: String], body: Data?) {
        self.init(method: method, path: path, headers: headers, body: body, mode: .receipt)
    }

    private init(method: String, path: String, headers: [String: String], body: Data?, mode: Mode,
                 disk: R3UploadSource? = nil) {
        self.method = method
        self.path = path
        self.headers = headers
        self.body = body
        self.mode = mode
        self.disk = disk
    }

    public static func stagedSave(path: String, headers: [String: String], source: R3UploadSource) -> Self {
        Self(method: "POST", path: path, headers: headers, body: nil, mode: .receipt, disk: source)
    }

    private static let root = "/zxs/v1/containers"

    public static func catalog(after: String?) throws -> Self {
        var headers = ["zxs-view": "compact"]
        if let after {
            guard validID(after[...]) else { throw TransportFailure.invalidRequest }
            headers["zxs-after"] = after
        }
        return Self(method: "GET", path: root, headers: headers, body: nil, mode: .metadata)
    }

    public static func detail(container: String, sequence: UInt64?, after: String?) throws -> Self {
        guard validID(container[...]) else { throw TransportFailure.invalidRequest }
        var headers = ["zxs-detail": "paged"]
        if let sequence {
            guard sequence > 0 else { throw TransportFailure.invalidRequest }
            headers["zxs-detail-sequence"] = String(sequence)
        }
        if let after {
            guard sequence != nil, after == "main" || validID(after[...]) else { throw TransportFailure.invalidRequest }
            headers["zxs-detail-after"] = after
        }
        return Self(method: "GET", path: root + "/" + container, headers: headers, body: nil, mode: .metadata)
    }

    public static func history(container: String, branch: String, head: String, next: String?) throws -> Self {
        guard validID(container[...]), branch == "main" || validID(branch[...]), validID(head[...])
        else { throw TransportFailure.invalidRequest }
        var headers = ["zxs-history-head": head]
        if let next {
            guard validID(next[...]), next != head else { throw TransportFailure.invalidRequest }
            headers["zxs-history-next"] = next
        }
        return Self(method: "GET", path: root + "/" + container + "/branches/" + branch + "/nodes",
                    headers: headers, body: nil, mode: .metadata)
    }

    public static func textNode(container: String, node: String, length: Int, sha256: String) throws -> Self {
        guard validID(container[...]), validID(node[...]), (0...256 * 1024).contains(length),
              sha256.utf8.count == 64, hex(sha256.utf8) else { throw TransportFailure.invalidRequest }
        return Self(method: "GET", path: root + "/" + container + "/nodes/" + node + "/data",
                    headers: [:], body: nil, mode: .node(length: length, digest: sha256))
    }

    func frames(authority: String, authorization: () throws -> String) throws -> [HTTP2Frame.FramePayload] {
        try validateRequest()
        let credential = try authorization()
        guard (1...8192).contains(credential.utf8.count), Self.headerText(credential) else {
            throw TransportFailure.invalidRequest
        }
        let accepted: String
        switch mode {
        case .node: accepted = "application/octet-stream"
        case .receipt, .metadata: accepted = "application/json"
        }
        var fields = HPACKHeaders([
            (":method", method), (":scheme", "https"), (":authority", authority), (":path", path),
            ("accept", accepted), ("accept-encoding", "identity"),
        ])
        fields.add(name: "authorization", value: credential, indexing: .neverIndexed)
        for (name, value) in headers.sorted(by: { $0.key < $1.key }) {
            fields.add(name: name.lowercased(), value: value)
        }

        let empty = disk == nil && (body?.isEmpty ?? true)
        let head = HTTP2Frame.FramePayload.headers(.init(headers: fields, endStream: empty))
        guard !empty, let body else { return [head] }
        var buffer = ByteBufferAllocator().buffer(capacity: body.count)
        buffer.writeBytes(body)
        return [head, .data(.init(data: .byteBuffer(buffer), endStream: true))]
    }

    private func validateRequest() throws {
        // Non-receipt reads are only constructed by the typed route builders above.
        guard case .receipt = mode else { return }
        let fields = path.split(separator: "/", omittingEmptySubsequences: false)
        guard fields.count >= 7, fields[0] == "", fields[1] == "zxs", fields[2] == "v1",
              fields[3] == "containers", Self.validID(fields[4]) else { throw TransportFailure.invalidRequest }

        if method == "GET" {
            guard fields.count == 7, fields[5] == "operations", Self.validID(fields[6]),
                  headers.isEmpty, body == nil, disk == nil else { throw TransportFailure.invalidRequest }
            return
        }
        guard method == "POST", fields.count == 8, fields[5] == "branches", fields[7] == "nodes",
              fields[6] == "main" || Self.validID(fields[6]) else { throw TransportFailure.invalidRequest }
        let length: Int, digest: String
        switch (body, disk) {
        case let (body?, nil):
            guard body.count <= 256 * 1024 else { throw TransportFailure.invalidRequest }
            length = body.count
            digest = SHA256.hash(data: body).map { String(format: "%02x", $0) }.joined()
        case let (nil, source?):
            guard source.length >= 0, source.sha256.utf8.count == 64, Self.hex(source.sha256.utf8)
            else { throw TransportFailure.invalidRequest }
            length = source.length; digest = source.sha256
        default: throw TransportFailure.invalidRequest
        }

        let required: Set<String> = ["content-type", "content-length", "zxs-operation", "zxs-node", "zxs-sha256"]
        let optional: Set<String> = ["zxs-base-node", "zxs-initial-name", "zxs-fork-from-main"]
        var normalized: [String: String] = [:]
        for (key, value) in headers {
            let name = key.lowercased()
            guard key.utf8.allSatisfy({ $0 < 128 }), required.contains(name) || optional.contains(name),
                  normalized.updateValue(value, forKey: name) == nil, Self.headerText(value)
            else { throw TransportFailure.invalidRequest }
        }
        guard normalized["content-type"] == "application/octet-stream",
              normalized["content-length"] == String(length), normalized["zxs-sha256"] == digest,
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
    let mode: R3Request.Mode
    init(mode: R3Request.Mode = .receipt) { self.mode = mode }

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
              lengths[0].utf8.count <= 7, let length = Int(lengths[0]), String(length) == lengths[0],
              length >= 0 else { throw TransportFailure.invalidResponse }
        switch (mode, value) {
        case let (.node(expectedLength, digest), 200):
            guard length == expectedLength, all["content-type"] == ["application/octet-stream"],
                  all["zxs-sha256"] == [digest] else { throw TransportFailure.invalidResponse }
        case (.metadata, 200):
            guard length <= 1024 * 1024, all["content-type"] == ["application/json"]
            else { throw TransportFailure.invalidResponse }
        case (.receipt, 200), (.receipt, 404):
            guard length <= 2048, all["content-type"] == ["application/json"]
            else { throw TransportFailure.invalidResponse }
        default:
            guard length <= 2048 else { throw TransportFailure.invalidResponse }
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
        if status == 200, case let .node(_, digest) = mode {
            let actual = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
            guard digest == actual else { throw TransportFailure.invalidResponse }
        }
        ended = true
        return R3Response(status: status, bytes: bytes)
    }
}

final class R3ResponseReceiver: ChannelInboundHandler {
    typealias InboundIn = HTTP2Frame.FramePayload

    private let result: EventLoopPromise<R3Response>
    private let fileProof: FileSendProof?
    private var pending: R3ResponseAccumulator
    private(set) var finished = false

    init(result: EventLoopPromise<R3Response>, mode: R3Request.Mode, fileProof: FileSendProof? = nil) {
        self.result = result; self.fileProof = fileProof
        self.pending = R3ResponseAccumulator(mode: mode)
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        guard !finished else { return }
        do {
            let resolved: R3Response?
            switch unwrapInboundIn(data) {
            case let .headers(head):
                // An early error can inform the caller, but an early 200 is
                // not a receipt for this transfer. Retrying uses an independent GET.
                if let fileProof, !fileProof.completed, head.headers[":status"].contains("200") {
                    throw TransportFailure.invalidResponse
                }
                resolved = try pending.head(head.headers, end: head.endStream)
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
