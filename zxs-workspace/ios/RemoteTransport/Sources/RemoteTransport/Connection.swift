import Darwin
import Foundation
import Network
import NIOCore
import NIOHTTP2
import NIOTransportServices
import Security

enum TransportFailure: Error {
    case invalidOrigin, invalidALPN, serverStream, invalidRequest, invalidResponse, timeout, interrupted
}

struct TransportAddress: Sendable {
    let host: String
    let port: Int
    var authority: String { (host.contains(":") ? "[\(host)]" : host) + ":\(port)" }

    init(host: String, port: Int) throws {
        // Network.framework receives a host, not a URL, DNS search expression,
        // or credential. The app onboarding layer owns origin parsing.
        guard (1...65535).contains(port), (1...253).contains(host.utf8.count),
              host.utf8.allSatisfy({ byte in
                  (48...57).contains(byte) || (65...90).contains(byte)
                      || (97...122).contains(byte) || [45, 46, 58].contains(byte)
              }),
              host.utf8.contains(where: { $0 != 46 }) else { throw TransportFailure.invalidOrigin }
        if host.utf8.contains(58) {
            var address = in6_addr()
            guard host.withCString({ inet_pton(AF_INET6, $0, &address) }) == 1 else {
                throw TransportFailure.invalidOrigin
            }
        } else if host.utf8.allSatisfy({ (48...57).contains($0) || $0 == 46 }) {
            var address = in_addr()
            guard host.withCString({ inet_pton(AF_INET, $0, &address) }) == 1 else {
                throw TransportFailure.invalidOrigin
            }
        } else {
            let bare = host.hasSuffix(".") ? host.dropLast() : host[...]
            let labels = bare.split(separator: ".", omittingEmptySubsequences: false)
            guard !labels.isEmpty, labels.allSatisfy({
                (1...63).contains($0.utf8.count) && $0.utf8.first != 45 && $0.utf8.last != 45
            }) else { throw TransportFailure.invalidOrigin }
        }
        self.host = host.lowercased()
        self.port = port
    }
}

// No AUTH or body interface yet. The transport can only hand back a live
// channel attested by the TLS metadata of that same channel, without fallback.
struct NegotiatedHTTP2 {
    private let channel: Channel
    private let multiplexer: NIOHTTP2Handler.StreamMultiplexer

    static func connect(_ address: TransportAddress, group: NIOTSEventLoopGroup)
        -> EventLoopFuture<NegotiatedHTTP2> {
        let tls = NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
        sec_protocol_options_add_tls_application_protocol(tls.securityProtocolOptions, "h2")
        // Keep the platform trust/hostname verifier. There is no custom verify
        // block, no alternate endpoint, and no application write on bootstrap.
        return NIOTSConnectionBootstrap(group: group)
            .connectTimeout(.seconds(10))
            .channelOption(ChannelOptions.autoRead, value: false)
            .tlsOptions(tls)
            .connect(host: address.host, port: address.port)
            .flatMap { channel in
                let verified = channel.getMetadata(definition: NWProtocolTLS.definition).flatMapThrowing { metadata in
                    guard let tlsMetadata = metadata as? NWProtocolTLS.Metadata,
                          let token = sec_protocol_metadata_copy_negotiated_protocol(tlsMetadata.securityProtocolMetadata)
                    else { throw TransportFailure.invalidALPN }
                    defer { free(UnsafeMutableRawPointer(mutating: token)) }
                    guard matchesHTTP2(String(validatingCString: token)) else { throw TransportFailure.invalidALPN }
                }
                return verified.flatMap {
                    var settings = NIOHTTP2Handler.ConnectionConfiguration()
                    settings.initialSettings.removeAll { $0.parameter == .enablePush }
                    settings.initialSettings.append(.init(parameter: .enablePush, value: 0))
                    return channel.configureHTTP2Pipeline(
                        mode: .client,
                        connectionConfiguration: settings,
                        streamConfiguration: .init(),
                        inboundStreamInitializer: { $0.eventLoop.makeFailedFuture(TransportFailure.serverStream) })
                }.flatMap { multiplexer in
                    channel.setOption(ChannelOptions.autoRead, value: true).map {
                        NegotiatedHTTP2(channel: channel, multiplexer: multiplexer)
                    }
                }.flatMapError { error in
                    channel.close(promise: nil)
                    return channel.eventLoop.makeFailedFuture(error)
                }
            }
    }

    static func matchesHTTP2(_ negotiated: String?) -> Bool { negotiated == "h2" }

    static func sendOnce(_ request: R3Request, address: TransportAddress, group: NIOTSEventLoopGroup,
                         permission: SendPermission, authorization: @escaping @Sendable () throws -> String) -> EventLoopFuture<R3Response> {
        connect(address, group: group).flatMap { connection in
            let prepared = connection.channel.eventLoop.makeCompletedFuture {
                // No material from the credential supplier is evaluated until
                // this exact TLS channel proved h2 and installed the H2 codec.
                guard connection.channel.isActive else { throw TransportFailure.interrupted }
                return try permission.use {
                    try request.frames(authority: address.authority, authorization: authorization)
                }
            }
            return prepared.flatMap { frames in
                connection.exchange(frames: frames, mode: request.mode, permission: permission)
            }.always { _ in
                connection.channel.close(promise: nil)
            }
        }
    }

    private func exchange(frames: [HTTP2Frame.FramePayload], mode: R3Request.Mode, permission: SendPermission) -> EventLoopFuture<R3Response> {
        let loop = channel.eventLoop
        let promise = loop.makePromise(of: R3Response.self)
        let receiver = R3ResponseReceiver(result: promise, mode: mode)
        let timeout = loop.scheduleTask(in: .seconds(30)) {
            receiver.fail(TransportFailure.timeout)
        }
        multiplexer.createStreamChannel { stream in
            stream.pipeline.addHandler(receiver)
        }.whenComplete { outcome in
            switch outcome {
            case let .failure(error): receiver.fail(error)
            case let .success(stream):
                guard !receiver.finished else { stream.close(promise: nil); return }
                do {
                    try permission.use {
                        guard stream.isActive else { throw TransportFailure.interrupted }
                        for frame in frames.dropLast() { stream.write(frame, promise: nil) }
                        if let last = frames.last {
                            stream.writeAndFlush(last).whenFailure { receiver.fail($0) }
                        }
                    }
                } catch { receiver.fail(error) }
            }
        }
        return promise.futureResult.always { _ in timeout.cancel() }
    }

    func close() -> EventLoopFuture<Void> { channel.close() }
}
