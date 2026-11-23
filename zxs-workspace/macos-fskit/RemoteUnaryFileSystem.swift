import Darwin
import Foundation
import FSKit

/// Actual unary extension instance. No credential, remote call, path discovery or
/// pre-wired in-process object shared with the Qt host.
@available(macOS 27.0, *)
final class RemoteUnaryFileSystem: FSUnaryFileSystem, FSUnaryFileSystemOperations {
    private let gate = NSLock()
    private var claimed: String?, uncertainTokens = Set<String>(), unloading = false
    private var loaded: (RegisteredWork, RemoteDataVolume)?

    func probeResource(resource: FSResource,
                       replyHandler reply: @escaping @Sendable (FSProbeResult?, (any Error)?) -> Void) {
        do {
            guard let token = try RegisteredWork.token(for:resource) else { reply(.notRecognized,nil); return }
            gate.lock(); let available = claimed == nil && !uncertainTokens.contains(token) && uncertainTokens.count < 128; gate.unlock()
            guard available else { reply(.notRecognized,nil); return }
            let registration = try RegisteredWork.attempted(token:token)
            reply(.usable(name: "ZXS Workspace", containerID: FSContainerIdentifier(uuid: registration.ticket.volume)), nil)
        } catch { reply(nil, error) }
    }

    func loadResource(resource: FSResource, options: FSTaskOptions,
                      replyHandler reply: @escaping @Sendable (FSVolume?, (any Error)?) -> Void) {
        do {
            guard options.taskOptions.isEmpty || options.taskOptions == ["-o","noexec,nodev,nosuid,owners"] else {
                throw workError(ENOTSUP) // Reject forced, read-only and unrecognized variants; never upgrade rights.
            }
            guard let path=resource as? FSPathURLResource, let token = try RegisteredWork.token(for:path) else { throw workError(EINVAL) }
            let registration = try RegisteredWork.attempted(token:token), ticket = registration.ticket
            gate.lock()
            let fresh = claimed == nil && !uncertainTokens.contains(token) && uncertainTokens.count < 128
            if fresh { claimed = token }
            gate.unlock()
            guard fresh else { throw workError(EBUSY) }
            var accepted = false
            defer {
                if !accepted {
                    gate.lock(); uncertainTokens.insert(token); claimed = nil; gate.unlock()
                }
            }
            guard try ticket.marked(.attempted), try !ticket.marked(.verified), try !ticket.marked(.stop)
            else { throw workError(ESTALE) }
            try ticket.mark(.loaded) // Cross-instance single-use admission; failure never invokes another attachment.
            let names = try registration.loadNames()
            let volume = try RemoteDataVolume(ticket: ticket, resource:path, name: FSFileName(data: Data("ZXS Workspace".utf8)), names: names)
            gate.lock()
            loaded = (registration,volume); accepted = true; containerStatus = .ready
            gate.unlock()
            reply(volume, nil)
        } catch { reply(nil, error) }
    }

    func unloadResource(resource: FSResource, options: FSTaskOptions,
                        replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            guard let path = resource as? FSPathURLResource, !path.isWritable else { throw workError(EINVAL) }
            guard options.taskOptions.isEmpty else { throw workError(ENOTSUP) }
            gate.lock()
            guard let (registration, volume) = loaded, registration.ticket.matches(path.url), !unloading else { gate.unlock(); throw workError(EBUSY) }
            unloading = true; gate.unlock()
            do {
                try registration.check()
                try volume.acknowledgeUnload()
            } catch {
                gate.lock(); unloading = false; gate.unlock()
                throw error // Still own the original. Never drop a live/misordered volume.
            }
            gate.lock(); loaded = nil; unloading = false; claimed = nil
            containerStatus = .notReady(status:workError(EAGAIN)); gate.unlock()
            reply(nil)
        } catch { reply(error) }
    }

}
