import ExtensionFoundation
import FSKit

/// Entry only for the separately provisioned macOS 27 FSKit appex target.
@available(macOS 27.0, *)
@main
struct ZxsRemoteExtension: UnaryFileSystemExtension {
    let fileSystem = RemoteUnaryFileSystem()
}
