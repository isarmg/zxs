import Darwin
import Foundation
import FSKit

/// A single original reference; actor state admits just one normal unmount worker at a time.
private final class MountEvidence: @unchecked Sendable {
    let raw: OpaquePointer
    init(_ p: OpaquePointer) { raw=p }
    deinit { zxs_mount_release(raw) }
}

/// One reviewed mount attempt in the signed foreground host, not a CLI, daemon
/// or installer. The controller must retain this object even when the outcome is
/// unknown. Deinit only closes metadata handles; it must never detach by itself.
@available(macOS 27.0, *)
@MainActor
final class OriginalMountHost {
    enum State { case ready, checking, uncertain, mounted, stopping, normalGone }
    private(set) var state = State.ready
    private let registration: RegisteredWork
    private var ticket: HeldMountTicket { registration.ticket }
    private var anchor: MountEvidence?

    init(registration: RegisteredWork) { self.registration = registration }

    func attach(confirmed: Bool) async throws -> URL {
        guard confirmed, state == .ready else { throw workError(EPERM) }
        state = .checking
        do {
            try await HostSigning.checkInstalled()
            try registration.check()
            for step in [HeldMountTicket.Step.attempted, .loaded, .verified, .stop, .stopped, .gone, .clean] {
                guard try !ticket.marked(step) else { throw workError(EEXIST) }
            }
            guard let before = zxs_mount_before() else { throw workError(errno) }
            anchor = MountEvidence(before)
            state = .uncertain // An interrupted exclusive reservation or a later error is not a fresh attempt.
            try ticket.mark(.attempted)
            let resource = FSPathURLResource(url: ticket.url, writable: false)
            // Request standard protections; neither argv nor their acceptance is mount evidence.
            let url = try await FSClient.shared.mountSingleVolume(resource: resource, bundleID: HostSigning.moduleID,
                                                                options: ["-o","noexec,nodev,nosuid,owners"])
            try await HostSigning.checkInstalled()
            try registration.check()
            var uuid = ticket.volume.uuid
            guard url.isFileURL, url.host == nil || url.host == "", url.query == nil, url.fragment == nil
            else { throw workError(ESTALE) }
            let e = url.withUnsafeFileSystemRepresentation { path -> Int32 in
                guard let path else { return EINVAL }
                return withUnsafeBytes(of: &uuid) { bytes in
                    zxs_mount_bind(before, path, bytes.bindMemory(to: UInt8.self).baseAddress,
                                   FSItem.Identifier.rootDirectory.rawValue)
                }
            }
            guard e == 0 else { throw workError(e) }
            guard try ticket.marked(.loaded) else { throw workError(ESTALE) }
            try ticket.mark(.verified)
            let second = zxs_mount_check(before)
            guard second == 0 else { throw workError(second) }
            state = .mounted
            return url
        } catch {
            if state == .checking { state = .ready } // No reservation and no call to FSClient yet.
            throw error
        }
    }

    /// Both OS disappearance and the matching provider's orderly teardown are required;
    /// if the process disappears before either, retain the original and report unknown.
    func confirmedLocalTeardown() throws -> Bool {
        guard state == .normalGone else { return false }
        return try ticket.marked(.attempted) && ticket.marked(.loaded) && ticket.marked(.verified) &&
            ticket.marked(.stop) && ticket.marked(.stopped) && ticket.marked(.gone) && ticket.marked(.clean)
    }

    func checkStillMounted() throws {
        guard (state == .mounted || state == .stopping), let anchor else { throw workError(ESTALE) }
        let e=zxs_mount_live(anchor.raw)
        if e != 0 { state = .uncertain; throw workError(e) }
    }

    func requestStop(confirmed: Bool) throws {
        guard confirmed, state == .mounted, let anchor else { throw workError(EPERM) }
        state = .uncertain
        let e = zxs_mount_check(anchor.raw); guard e == 0 else { throw workError(e) }
        try ticket.mark(.stop) // Module must acknowledge stopping this same registration.
        state = .stopping
    }

    /// Called only on explicit normal unmount. Keep the original mounted/retained
    /// on EBUSY; never retry automatically, detach by a path, force, or discard.
    func normalUnmount(confirmed: Bool) async throws {
        guard confirmed, state == .stopping, let anchor else { throw workError(EPERM) }
        try registration.check()
        guard try ticket.marked(.attempted), try ticket.marked(.verified), try ticket.marked(.stop)
        else { state = .uncertain; throw workError(ESTALE) }
        guard try ticket.marked(.stopped) else { throw workError(EBUSY) }
        state = .uncertain
        let e = await Task.detached { zxs_mount_normal(anchor.raw) }.value
        if e == EBUSY { state = .stopping; throw workError(e) }
        guard e == 0 else { throw workError(e) }
        try ticket.mark(.gone)
        state = .normalGone // OS evidence only. Not a sealed generation or authority to send.
    }
}
