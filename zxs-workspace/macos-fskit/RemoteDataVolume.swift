import Darwin
import Foundation
import FSKit

/// Admitted selections only. The name index, work files and virtual inode identities
/// belong to precisely one resource load. There is no transport in any FSKit callback.
@available(macOS 27.0, *)
final class RemoteDataVolume: FSVolume, FSVolume.DataCacheHandler, FSVolume.ReadWriteHandler, @unchecked Sendable {
    let rootItem = FSItem()
    let names: NameIndex
    private let ticket: HeldMountTicket, resource: FSPathURLResource
    private let signals = DispatchQueue(label: "org.zipworkspace.desktop.remotefs.volume-state")
    private var events: DispatchSourceFileSystemObject?
    private let gate = NSLock()
    let lookupAndReclaim = NSLock(), rootAttributesLock = NSLock()
    private enum Phase { case new, active, preparing, mounted, draining, fencing, fenced, notified, inactive, completed, neverMounted, releasedWithoutMount, failed }
    private var phase = Phase.new, callbacks = 0
    private var localFinishStarted = false, completedGeneration: Data?, completedFence: Data?

    init(ticket: HeldMountTicket, resource: FSPathURLResource, name: FSFileName, names: NameIndex) throws {
        try ticket.check()
        guard !resource.isRevoked, !resource.isWritable, ticket.matches(resource.url), try ticket.marked(.attempted),
              try !ticket.marked(.verified), try !ticket.marked(.stop)
        else { throw workError(ESTALE) }
        try names.checkOwner(ticket)
        try WorkFile.bind(names.ordered.map { ($0.source.file, $0.id) })
        self.names = names; self.ticket = ticket; self.resource = resource
        super.init(volumeID: FSVolume.Identifier(uuid: ticket.volume), volumeName: name)
    }

    private func observeControl() throws {
        let watch = fcntl(ticket.directory.fileDescriptor, F_DUPFD_CLOEXEC, 0)
        guard watch >= 0 else { throw workError(errno) }
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: watch,
                     eventMask: [.write, .rename, .delete, .revoke], queue: signals)
        source.setEventHandler { [weak self] in self?.changedControl() }
        source.setCancelHandler { _ = Darwin.close(watch) } // Independent FD outlives handler and cancellation.
        state { events = source }; source.resume()
        signals.async { [weak self] in self?.changedControl() }
    }

    private func changedControl() {
        do {
            if state({ phase == .mounted }), try ticket.marked(.stop) {
                guard try ticket.marked(.verified) else { throw workError(ESTALE) }
                try stopNewFileOpens()
            }
            guard state({ phase == .draining }) else { return }
            guard !resource.isRevoked else { throw workError(EACCES) }
            for row in names.ordered where try !row.source.file.readyToFence() { return }
            let entered = state {
                guard phase == .draining, callbacks == 0 else { return false }
                phase = .fencing; return true
            }
            guard entered else { return }
            // No new refs or admitted in-flight work. Close writers only after every
            // original has passed the local drain and device barriers. Kernel unmount
            // and the later per-provider clean acknowledgement are still independent.
            let checkpoint=try finishLocalFilesAfterDrain()
            // The host may observe the rename before this call finishes its post-rename
            // barriers. Serialize its resulting FSKit unmount/deactivation with the entire
            // bounded marker publication; only a completed mark can ever authorize clean.
            try state {
                guard phase == .fencing, callbacks == 0 else { throw workError(EIO) }
                do {
                    try ticket.mark(.stopped,proof:checkpoint)
                    completedFence=try ticket.stopFence()
                    completedGeneration=checkpoint; phase = .fenced
                } catch { phase = .failed; throw error }
            }
        } catch { state { phase = .failed }; cancelObserver() }
    }

    private func cancelObserver() {
        let source = state { let s = events; events = nil; return s }
        source?.cancel()
    }
    deinit { events?.cancel() }

    // Called only by unloadResource, after the normal FSVolume deactivation. Do not
    // wait inside the FSKit teardown for the host's funmount call to return. No I/O
    // against the virtual volume: the provider still owns its original private APFS readers.
    func acknowledgeUnload() throws {
        let orderly = try state {
            guard callbacks == 0 else { throw workError(EBUSY) }
            if phase == .new || phase == .neverMounted {
                phase = .releasedWithoutMount; return false // No writable mount, no clean witness.
            }
            guard phase == .inactive, localFinishStarted else { throw workError(EBUSY) }
            return true
        }
        guard orderly else { cancelObserver(); return }
        try names.checkOwner(ticket)
        guard let checkpoint=state({ completedGeneration }), let fence=state({ completedFence }),
              try ticket.witness(.stopped)==checkpoint, try ticket.stopFence()==fence
        else { throw workError(ESTALE) }
        for r in names.ordered { _ = try names.row(r.item); try r.source.file.synchronize() }
        // Framework guarantees no further callbacks after an orderly unload; serialize
        // terminal local publication against already entered/error callbacks, never with a reply.
        try state {
            guard phase == .inactive, callbacks == 0 else { throw workError(EBUSY) }
            do { try ticket.mark(.clean,proof:fence); phase = .completed }
            catch { phase = .failed; throw error }
        }
        cancelObserver()
    }

    // Only these innocuous root metadata calls can bootstrap the host's event-only
    // root handle. Never return names or a body until the host has independently
    // bound and verified the actual returned FSKit mount and required restrictions.
    func visibility(admitting: Bool = false) throws {
        guard !resource.isRevoked, try ticket.marked(.verified) else { throw workError(EACCES) }
        if admitting, try ticket.marked(.stop) { throw workError(EBUSY) }
    }

    private func accepted(_ options: FSTaskOptions) -> Bool {
        options.taskOptions.isEmpty || options.taskOptions == ["-o", "noexec,nodev,nosuid,owners"]
    }

    private func state<T>(_ action: () throws -> T) rethrows -> T {
        gate.lock(); defer { gate.unlock() }
        return try action()
    }

    // No reply, directory packer, whole-file I/O or hash under the lifecycle gate.
    // Only bounded terminal metadata publication is serialized with teardown.
    func serve<T>(admitting: Bool = false, rootMetadata: Bool = false, fencedSync: Bool = false,
                  lateMutation: Bool = false, _ reply: (T?, (any Error)?) -> Void, _ run: () throws -> T) {
        let entered = state {
            if lateMutation && (phase == .fencing || phase == .fenced || phase == .notified || phase == .inactive) {
                phase = .failed; return false // Late write beyond our fence: never acknowledge a clean handoff.
            }
            guard phase == .mounted || (!admitting && phase == .draining) || (rootMetadata && phase == .active) ||
                    (((rootMetadata && !admitting) || fencedSync) && phase == .fenced) else { return false }
            callbacks += 1; return true
        }
        guard entered else { reply(nil, workError(EBUSY)); return }
        defer {
            let wake = state { callbacks -= 1; return phase == .draining && callbacks == 0 }
            if wake { signals.async { [weak self] in self?.changedControl() } }
        }
        do {
            if !rootMetadata { try visibility(admitting: admitting) }
            reply(try run(), nil)
        } catch { reply(nil, error) }
    }

    func activateVolume(options: FSTaskOptions,
                        replyHandler reply: @escaping @Sendable (FSActivateResult?, (any Error)?) -> Void) {
        do {
            guard accepted(options) else { throw workError(ENOTSUP) }
            try names.check()
            guard let result = FSActivateResult(rootItem: rootItem) else { throw workError(EIO) }
            try state { guard phase == .new else { throw workError(EBUSY) }; phase = .active }
            reply(result, nil)
        } catch { reply(nil, error) }
    }

    func mount(options: FSTaskOptions, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            guard accepted(options) else { throw workError(ENOTSUP) }
            try names.check()
            try state { guard phase == .active else { throw workError(EBUSY) }; phase = .preparing }
            do { try observeControl() }
            catch { state { phase = .active }; throw error }
            try state { guard phase == .preparing else { throw workError(EBUSY) }; phase = .mounted }
            signals.async { [weak self] in self?.changedControl() }
            reply(nil)
        } catch { reply(error) }
    }

    // A host must call this before asking the kernel for a *normal* unmount. Busy
    // leaves new admissions stopped and the originals held for a later normal attempt.
    private func stopNewFileOpens() throws {
        try state {
            guard phase == .mounted || phase == .draining else { throw workError(EBUSY) }
            phase = .draining
        }
        for r in names.ordered { r.source.file.stopNewOpens() }
    }

    func unmount(replyHandler reply: @escaping @Sendable () -> Void) {
        state { phase = (phase == .fenced && callbacks == 0) ? .notified : .failed }
        reply() // Notification only: this callback is void. No disk I/O, discard or receipt.
    }

    func deactivateVolume(options: FSDeactivateOptions, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            try state {
                if options.isEmpty, phase == .active, callbacks == 0 {
                    phase = .neverMounted // Unary unload may release only, never attest a clean handoff.
                    return
                }
                guard options.isEmpty, phase == .notified, callbacks == 0 else {
                    phase = .failed; throw workError(EBUSY)
                }
                phase = .inactive
            }
            reply(nil) // Already synced, unmounted and reclaimed. No private or mounted I/O in this callback.
        } catch { reply(error) }
    }

    func reclaimItem(_ item: FSItem, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        // No per-lookup descriptor. The original private owner survives kernel reclaim;
        // cached FSItem identity can be reused on a subsequent lookup during this volume.
        guard item === rootItem || names.issued(item) != nil else { reply(workError(ESTALE)); return }
        lookupAndReclaim.lock()
        _ = item.tryReclaim {} // Nothing per lookup to dispose; the resource owner holds the originals.
        lookupAndReclaim.unlock()
        reply(nil)
    }

    // Local half only; must still independently prove original normal unmount,
    // establish a durable owner/error checkpoint and verify a complete sealed body.
    private func finishLocalFilesAfterDrain() throws -> Data {
        guard try ticket.marked(.verified), try ticket.marked(.stop) else { throw workError(EBUSY) }
        try state {
            guard phase == .fencing, callbacks == 0, !localFinishStarted else { throw workError(EBUSY) }
            localFinishStarted = true
        }
        do {
            for r in names.ordered { try r.source.file.requireDrained() }
            for r in names.ordered { _ = try names.row(r.item); try r.source.file.synchronize() }
            for r in names.ordered { try r.source.file.closeLocalWriterAfterDrain() }
            // Hash here, not while the kernel is blocked in funmount/unload.
            let rows=try names.ordered.map { try $0.source.file.finalGeneration(container:$0.source.containerID) }
                .sorted { $0.container < $1.container }
            return try DrainedFiles(format:"zxs-fskit-stopped-1",token:ticket.token,digest:ticket.digest,entries:rows).encoded()
        } catch { state { phase = .failed }; throw error }
    }

    var isDataCacheInhibited: Bool { false }

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode,
              context: FSContext, replyHandler reply: @escaping @Sendable (FSOpenItemResult?, (any Error)?) -> Void) {
        serve(admitting: true, rootMetadata: item === rootItem, reply) {
            try WorkFile.authorize(context)
            let candidate: FSOpenItemResult? = FSOpenItemResult(grantedCoherency: .noCache)
            guard let result = candidate else { throw workError(EIO) }
            if item === rootItem {
                guard modes.isEmpty || modes == .read else { throw workError(EACCES) }
                try names.check()
            } else { try names.row(item).source.file.opened(modes) }
            return result
        }
    }

    func upgrade(_ item: FSItem, cacheMode: FSVolume.DataCacheMode, context: FSContext,
                 replyHandler reply: @escaping @Sendable (FSUpgradeItemResult?, (any Error)?) -> Void) {
        serve(admitting: true, reply) {
            try WorkFile.authorize(context)
            let candidate: FSUpgradeItemResult? = FSUpgradeItemResult(grantedCoherency: .noCache)
            guard let result = candidate else { throw workError(EIO) }
            if item === rootItem { try names.check() }
            else { try names.row(item).source.file.cacheUpgrade() }
            return result
        }
    }

    func close(_ item: FSItem, context: FSContext, replyHandler reply: @escaping @Sendable () -> Void) {
        guard let original = names.issued(item) else { reply(); return }
        serve({ (_: Bool?, e: (any Error)?) in if let e { original.source.file.closed(e) }; reply() }) {
            try WorkFile.authorize(context)
            guard try names.row(item) === original else { throw workError(ESTALE) }
            original.source.file.closed()
            return true
        }
    }

    func read(from item: FSItem, at offset: off_t, length: Int, into buffer: FSMutableFileDataBuffer,
              replyHandler reply: @escaping @Sendable (FSReadFileResult?, (any Error)?) -> Void) {
        serve(reply) { try names.row(item).source.file.read(at: offset, count: length, buffer: buffer) }
    }

    func write(contents: Data, to item: FSItem, at offset: off_t,
               replyHandler reply: @escaping @Sendable (FSWriteFileResult?, (any Error)?) -> Void) {
        guard names.issued(item) != nil else { reply(nil, workError(ESTALE)); return }
        serve(lateMutation: true, reply) { try names.row(item).source.file.write(contents, at: offset) }
    }

    func setAttributes(_ request: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext,
                       replyHandler reply: @escaping @Sendable (FSSetAttributesResult?, (any Error)?) -> Void) {
        request.consumedAttributes = []
        do { try WorkFile.authorize(context) } catch { reply(nil, error); return }
        guard item === rootItem || names.issued(item) != nil else { reply(nil, workError(ESTALE)); return }
        serve(admitting: true, lateMutation: item !== rootItem, reply) {
            let readonly: [FSItem.Attribute] = [.fileID, .parentID, .type, .linkCount, .allocSize]
            guard !readonly.contains(where: { request.isValid($0) }) else { throw workError(EINVAL) }
            let unsupported: [FSItem.Attribute] = [.mode, .uid, .gid, .flags, .accessTime, .modifyTime,
                .changeTime, .birthTime, .backupTime, .addedTime, .supportsLimitedXAttrs, .inhibitKernelOffloadedIO]
            guard request.isValid(.size), !unsupported.contains(where: { request.isValid($0) }) else {
                throw workError(ENOTSUP)
            }
            if item === rootItem {
                let a = try rootAttributes(FSSetAttributesResult.requestedAttributes)
                guard let response = FSSetAttributesResult(attributes: a, freeSpace: nil) else { throw workError(EIO) }
                return response // FSKit ignores directory size: do not mark it consumed.
            }
            let r = try names.row(item).source.file.resize(request.size)
            request.consumedAttributes = .size
            return r
        }
    }

    func synchronize(flags: FSSyncFlags, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        serve(fencedSync: true, { (_: Bool?, e: (any Error)?) in reply(e) }) {
            var first: Error?
            for r in names.ordered {
                do { _ = try names.row(r.item); try r.source.file.synchronize() }
                catch { if first == nil { first = error } }
            }
            if let first { throw first }
            return true
        }
    }
}
