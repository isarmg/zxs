import Foundation
import NIOCore
import NIOTransportServices

public struct R3HTTPSOrigin: Sendable {
    let address: TransportAddress
    public var identity: String { "https://" + address.authority }

    public init(_ input: String) throws {
        guard input.hasPrefix("https://"), input.utf8.count <= 272,
              input.utf8.allSatisfy({ (33...126).contains($0) }),
              !input.contains("%"), !input.contains("\\"),
              let components = URLComponents(string: input), components.scheme == "https",
              components.user == nil, components.password == nil,
              components.percentEncodedQuery == nil, components.percentEncodedFragment == nil,
              components.path == "" || components.path == "/", var host = components.host else {
            throw TransportFailure.invalidOrigin
        }
        let authority = components.path == "/" ? input.dropLast() : input[...]
        guard !authority.hasSuffix(":") else { throw TransportFailure.invalidOrigin }
        if host.hasPrefix("[") && host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        address = try TransportAddress(host: host, port: components.port ?? 443)
    }
}

final class SendPermission: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false

    func cancel() { lock.lock(); cancelled = true; lock.unlock() }
    func use<T>(_ action: () throws -> T) throws -> T {
        lock.lock(); defer { lock.unlock() }
        guard !cancelled else { throw CancellationError() }
        return try action()
    }
}

public final class R3HTTPSClient: Sendable {
    private let group = NIOTSEventLoopGroup(loopCount: 1)

    public init() {}

    deinit { group.shutdownGracefully(queue: .global()) { _ in } }

    public func once(_ request: R3Request, to origin: R3HTTPSOrigin,
                     authorization: @escaping @Sendable () throws -> String) async throws -> R3Response {
        let permission = SendPermission()
        return try await withTaskCancellationHandler(operation: {
            try Task.checkCancellation()
            return try await NegotiatedHTTP2.sendOnce(request, address: origin.address, group: group,
                                                      permission: permission, authorization: authorization).get()
        }, onCancel: {
            permission.cancel()
        })
    }
}
