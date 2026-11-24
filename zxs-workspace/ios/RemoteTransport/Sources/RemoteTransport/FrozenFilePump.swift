import CryptoKit
import Foundation
import NIOCore
import NIOHTTP2

// Accessed only on the same HTTP/2 stream event loop as the pump and receiver.
final class FileSendProof {
    private(set) var completed = false
    func endWasWritten() { completed = true }
}

struct VerifiedFileChunks {
    enum Step { case bytes(Data), checkedEnd }

    private let length: Int
    private let expected: String
    private var consumed = 0
    private var digest = SHA256()
    private var ended = false
    private static let chunk = 16_384

    init(length: Int, sha256: String) { self.length = length; self.expected = sha256 }

    var demand: Int {
        consumed < length ? min(Self.chunk, length - consumed) : 1
    }

    mutating func accept(_ next: Data?) throws -> Step {
        guard !ended, length >= 0 else { throw TransportFailure.invalidRequest }
        let data = next ?? Data()
        if consumed < length {
            guard !data.isEmpty, data.count <= demand else { throw TransportFailure.invalidRequest }
            consumed += data.count
            digest.update(data: data)
            return .bytes(data)
        }
        guard data.isEmpty, digest.finalize().map({ String(format: "%02x", $0) }).joined() == expected
        else { throw TransportFailure.invalidRequest }
        ended = true
        return .checkedEnd
    }
}

// All mutable pump state belongs to the stream event loop. The worker only
// reads from the immutable, caller-owned source and dispatches one result back.
final class FrozenFilePump: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = HTTP2Frame.FramePayload

    private let source: R3UploadSource
    private let permission: SendPermission
    private let receiver: R3ResponseReceiver
    private let proof: FileSendProof
    private weak var context: ChannelHandlerContext?
    private var chunks: VerifiedFileChunks
    private var pending: VerifiedFileChunks.Step?
    private var inRead = false, inWrite = false, started = false, sealed = false
    private var idle: Scheduled<Void>?

    init(source: R3UploadSource, permission: SendPermission, receiver: R3ResponseReceiver, proof: FileSendProof) {
        self.source = source; self.permission = permission; self.receiver = receiver; self.proof = proof
        self.chunks = VerifiedFileChunks(length: source.length, sha256: source.sha256)
    }

    func handlerAdded(context: ChannelHandlerContext) { self.context = context }
    func handlerRemoved(context: ChannelHandlerContext) { stop(); self.context = nil }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        context.fireChannelRead(data)
    }

    func channelWritabilityChanged(context: ChannelHandlerContext) {
        advance()
        context.fireChannelWritabilityChanged()
    }

    func channelInactive(context: ChannelHandlerContext) {
        stop()
        context.fireChannelInactive()
    }

    func armIdleTimeout() {
        guard let context, !receiver.finished else { return }
        idle?.cancel()
        idle = context.eventLoop.scheduleTask(in: .seconds(30)) { [weak self] in
            if let self { self.receiver.fail(TransportFailure.timeout) }
        }
    }

    func start() {
        started = true
        armIdleTimeout()
        advance()
    }

    func stop() {
        started = false
        idle?.cancel(); idle = nil
        pending = nil
    }

    private func advance() {
        guard let context, started, !sealed, !receiver.finished, !inRead, !inWrite,
              context.channel.isActive, context.channel.isWritable else { return }
        if let pending {
            self.pending = nil
            send(pending, on: context.channel)
            return
        }
        inRead = true
        let request = chunks.demand, loop = context.eventLoop
        let source = self.source, permission = self.permission
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Result<Data?, Error> {
                try permission.use {}
                return try source.read(request)
            }
            loop.execute { [weak self] in self?.readCompleted(result) }
        }
    }

    private func readCompleted(_ result: Result<Data?, Error>) {
        inRead = false
        guard started, !sealed, !receiver.finished else { return }
        do {
            pending = try chunks.accept(result.get())
            armIdleTimeout()
            advance()
        } catch { receiver.fail(error) }
    }

    private func send(_ step: VerifiedFileChunks.Step, on stream: Channel) {
        let end: Bool
        var buffer = ByteBufferAllocator().buffer(capacity: 0)
        switch step {
        case let .bytes(data): buffer.writeBytes(data); end = false
        case .checkedEnd: end = true
        }
        inWrite = true
        do {
            let future = try permission.use {
                guard stream.isActive else { throw TransportFailure.interrupted }
                return stream.writeAndFlush(HTTP2Frame.FramePayload.data(.init(data: .byteBuffer(buffer), endStream: end)))
            }
            future.whenComplete { [weak self] result in
                guard let self else { return }
                self.inWrite = false
                switch result {
                case .success:
                    self.sealed = end
                    if end { self.proof.endWasWritten() }
                    self.armIdleTimeout()
                    self.advance()
                case let .failure(error): self.receiver.fail(error)
                }
            }
        } catch { receiver.fail(error) }
    }
}
