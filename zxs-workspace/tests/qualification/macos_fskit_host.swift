// Nonexecuted source-only checks for a future signed, disposable native fixture.
// No entry point, discovery, installer, volume modifications or real mount call.
import Darwin
import Foundation
import FSKit

@available(macOS 27.0, *)
enum FSKitHostQualification {
    static func unrelatedResource(_ fs: RemoteUnaryFileSystem, registered: HeldMountTicket) async throws {
        // Not opened or inspected by the provider: only the literal original empty ticket can match.
        let wrong = registered.directoryURL.appendingPathComponent("not-the-ticket")
        let probe = try await fs.probeResource(resource: FSPathURLResource(url: wrong, writable: false))
        guard probe.result == .notRecognized else { throw workError(EIO) }
        let writable = try await fs.probeResource(resource: FSPathURLResource(url: registered.url, writable: true))
        guard writable.result == .notRecognized else { throw workError(EIO) }
    }

    @MainActor static func cannotAttachWithoutRequest(_ original: OriginalMountHost) async throws {
        guard original.state == .ready else { throw workError(EINVAL) }
        do { _ = try await original.attach(confirmed: false) }
        catch let e as NSError where e.domain == NSPOSIXErrorDomain && e.code == Int(EPERM) {
            guard original.state == .ready else { throw workError(EIO) }
            return
        }
        throw workError(EIO)
    }

    @MainActor static func requireIndependentReceipts(_ original: OriginalMountHost,
                                                      ticket: HeldMountTicket) throws {
        let both = try original.confirmedLocalTeardown()
        if both {
            guard original.state == .normalGone, try ticket.marked(.stopped), try ticket.marked(.gone),
                  try ticket.marked(.clean) else { throw workError(EIO) }
        } else if original.state == .normalGone, try ticket.marked(.clean) { throw workError(EIO) }
    }
}
