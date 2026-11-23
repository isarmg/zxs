// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "RemoteTransport",
    platforms: [.iOS(.v16), .macOS(.v12)],
    products: [.library(name: "RemoteTransport", targets: ["RemoteTransport"])],
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", exact: "2.105.1"),
        .package(url: "https://github.com/apple/swift-nio-http2.git", exact: "1.46.1"),
        .package(url: "https://github.com/apple/swift-nio-transport-services.git", exact: "1.28.0"),
        .package(url: "https://github.com/apple/swift-atomics.git", exact: "1.3.1"),
        .package(url: "https://github.com/apple/swift-collections.git", exact: "1.2.1"),
        .package(url: "https://github.com/apple/swift-system.git", exact: "1.4.2"),
    ],
    targets: [
        .target(name: "RemoteTransport", dependencies: [
            .product(name: "NIOCore", package: "swift-nio"),
            .product(name: "NIOHTTP2", package: "swift-nio-http2"),
            .product(name: "NIOTransportServices", package: "swift-nio-transport-services"),
        ]),
        .testTarget(name: "RemoteTransportTests", dependencies: ["RemoteTransport"]),
    ],
    swiftLanguageModes: [.v5]
)
