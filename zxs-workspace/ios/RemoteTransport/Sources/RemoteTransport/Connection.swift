import Darwin
import Network
import NIOCore
import NIOHTTP2
import NIOTransportServices
import Security

enum TransportFailure: Error { case invalidOrigin, invalidALPN, serverStream }

struct TransportAddress {
    let host: String
    let port: Int

    init(host: String, port: Int) throws {
        // Network.framework receives a host, not a URL, DNS search expression,
        // or credential. The app onboarding layer owns origin parsing.
        guard (1...65535).contains(port), (1...253).contains(host.utf8.count),
              host.utf8.allSatisfy({ byte in
                  (48...57).contains(byte) || (65...90).contains(byte)
                      || (97...122).contains(byte) || [45, 46, 58].contains(byte)
              }),
              host.utf8.contains(where: { $0 != 46 }) else { throw TransportFailure.invalidOrigin }
        self.host = host
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

    func close() -> EventLoopFuture<Void> { channel.close() }
}
