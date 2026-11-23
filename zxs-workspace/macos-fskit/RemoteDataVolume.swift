import Darwin
import Foundation
import FSKit

/// Admitted selections only. The name index, work files and virtual inode identities
/// belong to precisely one resource load. There is no transport in any FSKit callback.
@available(macOS 27.0, *)
final class RemoteDataVolume: FSVolume, FSVolume.DataCacheHandler, FSVolume.ReadWriteHandler {
    let rootItem = FSItem()
    let names: NameIndex
    private let gate = NSLock()
    let lookupAndReclaim = NSLock(), rootAttributesLock = NSLock()
    private enum Phase { case new, active, mounted, draining, notified, inactive, failed }
    private var phase = Phase.new, callbacks = 0
    private var localFinishStarted = false

    init(id: FSVolume.Identifier, name: FSFileName, names: NameIndex) throws {
        try WorkFile.bind(names.ordered.map { ($0.source.file, $0.id) })
        self.names = names
        super.init(volumeID: id, volumeName: name)
    }

    private func state<T>(_ action: () throws -> T) rethrows -> T {
        gate.lock(); defer { gate.unlock() }
        return try action()
    }

    // Never call an external reply, enumerate packer, or a potentially slow APFS
    // operation while holding the lifecycle gate. Reject unordered teardown.
    func serve<T>(admitting: Bool = false, _ reply: (T?, (any Error)?) -> Void, _ run: () throws -> T) {
        let entered = state {
            guard phase == .mounted || (!admitting && phase == .draining) else { return false }
            callbacks += 1; return true
        }
        guard entered else { reply(nil, workError(EBUSY)); return }
        defer { state { callbacks -= 1 } }
        do { reply(try run(), nil) } catch { reply(nil, error) }
    }

    func activateVolume(options: FSTaskOptions,
                        replyHandler reply: @escaping @Sendable (FSActivateResult?, (any Error)?) -> Void) {
        do {
            guard options.taskOptions.isEmpty else { throw workError(ENOTSUP) }
            try names.check()
            guard let result = FSActivateResult(rootItem: rootItem) else { throw workError(EIO) }
            try state { guard phase == .new else { throw workError(EBUSY) }; phase = .active }
            reply(result, nil)
        } catch { reply(nil, error) }
    }

    func mount(options: FSTaskOptions, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            guard options.taskOptions.isEmpty else { throw workError(ENOTSUP) }
            try names.check()
            try state { guard phase == .active else { throw workError(EBUSY) }; phase = .mounted }
            reply(nil)
        } catch { reply(error) }
    }

    // A host must call this before asking the kernel for a *normal* unmount. Busy
    // leaves new admissions stopped and the originals held for a later normal attempt.
    func stopNewFileOpens() throws {
        try state {
            guard phase == .mounted || phase == .draining else { throw workError(EBUSY) }
            phase = .draining
        }
        for r in names.ordered { r.source.file.stopNewOpens() }
    }

    func unmount(replyHandler reply: @escaping @Sendable () -> Void) {
        state { phase = (phase == .draining && callbacks == 0) ? .notified : .failed }
        reply() // Notification only: this callback is void. No disk I/O, discard or receipt.
    }

    func deactivateVolume(options: FSDeactivateOptions, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do {
            try state {
                if options.isEmpty, phase == .active, callbacks == 0 {
                    phase = .failed // Activated but never mounted: allow cleanup, never qualify local finish.
                    return
                }
                guard options.isEmpty, phase == .notified, callbacks == 0 else {
                    phase = .failed; throw workError(EBUSY)
                }
                phase = .inactive
            }
            reply(nil) // FSKit already synced, unmounted and reclaimed. No further I/O in this callback.
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
    func finishLocalFilesAfterDrain() throws {
        try state {
            guard phase == .inactive, callbacks == 0, !localFinishStarted else { throw workError(EBUSY) }
            localFinishStarted = true
        }
        do {
            for r in names.ordered { try r.source.file.requireDrained() }
            for r in names.ordered { _ = try names.row(r.item); try r.source.file.synchronize() }
            for r in names.ordered { try r.source.file.closeLocalWriterAfterDrain() }
        } catch { state { phase = .failed }; throw error }
    }

    var isDataCacheInhibited: Bool { false }

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode,
              context: FSContext, replyHandler reply: @escaping @Sendable (FSOpenItemResult?, (any Error)?) -> Void) {
        serve(admitting: true, reply) {
            try WorkFile.authorize(context)
            let candidate: FSOpenItemResult? = FSOpenItemResult(grantedCoherency: .noCache)
            guard let result = candidate else { throw workError(EIO) }
            if item === rootItem {
                guard modes == .read else { throw workError(EACCES) }
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
        serve(reply) { try names.row(item).source.file.write(contents, at: offset) }
    }

    func setAttributes(_ request: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext,
                       replyHandler reply: @escaping @Sendable (FSSetAttributesResult?, (any Error)?) -> Void) {
        request.consumedAttributes = []
        serve(admitting: true, reply) {
            try WorkFile.authorize(context)
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
        serve({ (_: Bool?, e: (any Error)?) in reply(e) }) {
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
