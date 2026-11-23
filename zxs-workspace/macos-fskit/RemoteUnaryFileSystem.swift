import Darwin
import Foundation
import FSKit

/// Private registered-volume instance for the signed extension. No path discovery:
/// the signed issuer supplies original group objects from the approved checkout.
@available(macOS 27.0, *)
final class RemoteUnaryFileSystem: FSUnaryFileSystem, FSUnaryFileSystemOperations {
    private let gate = NSLock(), ticket: HeldMountTicket, names: NameIndex
    private var used = false, unloading = false
    private var loaded: RemoteDataVolume?

    init(ticket: HeldMountTicket, names: NameIndex) throws {
        try HostSigning.checkRunningModule()
        try names.checkOwner(ticket)
        self.ticket = ticket; self.names = names
        super.init()
    }

    private func exact(_ resource: FSResource) throws -> FSPathURLResource {
        guard let item = resource as? FSPathURLResource, !item.isWritable, ticket.matches(item.url)
        else { throw workError(EINVAL) }
        return item
    }

    func probeResource(resource: FSResource,
                       replyHandler reply: @escaping @Sendable (FSProbeResult?, (any Error)?) -> Void) {
        guard (try? exact(resource)) != nil else { reply(.notRecognized, nil); return }
        // Ordinary URLs from this pre-authorized App Group are not security-scoped bookmarks.
        // Never obtain additional path permissions or inspect unrelated resources.
        gate.lock(); let available = !used; gate.unlock()
        guard available else { reply(.notRecognized,nil); return }
        do {
            try ticket.check()
            guard try ticket.marked(.attempted), try !ticket.marked(.loaded),
                  try !ticket.marked(.verified), try !ticket.marked(.stop) else { throw workError(ESTALE) }
            reply(.usable(name: "ZXS Workspace", containerID: FSContainerIdentifier(uuid: ticket.volume)), nil)
        } catch { reply(nil, error) }
    }

    func loadResource(resource: FSResource, options: FSTaskOptions,
                      replyHandler reply: @escaping @Sendable (FSVolume?, (any Error)?) -> Void) {
        do {
            guard options.taskOptions.isEmpty || options.taskOptions == ["-o","noexec,nodev,nosuid,owners"] else {
                throw workError(ENOTSUP) // Reject forced, read-only and unrecognized variants; never upgrade rights.
            }
            _ = try exact(resource)
            try ticket.check()
            gate.lock()
            let fresh = !used
            if fresh { used = true }
            gate.unlock()
            guard fresh, try ticket.marked(.attempted), try !ticket.marked(.verified), try !ticket.marked(.stop)
            else { throw workError(ESTALE) }
            try ticket.mark(.loaded) // Cross-instance single-use admission; failure never invokes another attachment.
            let volume = try RemoteDataVolume(ticket: ticket, name: FSFileName(data: Data("ZXS Workspace".utf8)), names: names)
            gate.lock()
            loaded = volume; containerStatus = .ready
            gate.unlock()
            reply(volume, nil)
        } catch { reply(nil, error) }
    }

    func unloadResource(resource: FSResource, options: FSTaskOptions,
                        replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            _ = try exact(resource)
            guard options.taskOptions.isEmpty else { throw workError(ENOTSUP) }
            gate.lock()
            guard let volume = loaded, !unloading else { gate.unlock(); throw workError(EBUSY) }
            unloading = true; gate.unlock()
            do {
                try ticket.check()
                try volume.acknowledgeUnload()
            } catch {
                gate.lock(); unloading = false; gate.unlock()
                throw error // Still own the original. Never drop a live/misordered volume.
            }
            gate.lock(); loaded = nil; unloading = false; gate.unlock()
            reply(nil)
        } catch { reply(error) }
    }

}
