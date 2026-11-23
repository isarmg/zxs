import Darwin
import Foundation
import FSKit

/// Private data callbacks for the one admitted work item. Not a registered extension:
/// the namespace + verified resource loader must add FSVolume.Handler before any native mount.
@available(macOS 27.0, *)
final class RemoteDataVolume: FSVolume, FSVolume.DataCacheHandler, FSVolume.ReadWriteHandler {
    let fileItem = FSItem()
    private let file: WorkFile

    init(id: FSVolume.Identifier, name: FSFileName, file: WorkFile) throws {
        try file.bindOnce()
        self.file = file
        super.init(volumeID: id, volumeName: name)
    }

    private func exact(_ item: FSItem) throws {
        guard item === fileItem else { throw workError(ESTALE) }
    }

    var isDataCacheInhibited: Bool { false }

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode,
              context: FSContext, replyHandler reply: @escaping @Sendable (FSOpenItemResult?, (any Error)?) -> Void) {
        do {
            try exact(item); try file.authorize(context); try file.opened(modes)
            reply(FSOpenItemResult(grantedCoherency: .noCache), nil)
        } catch { reply(nil, error) }
    }

    func upgrade(_ item: FSItem, cacheMode: FSVolume.DataCacheMode, context: FSContext,
                 replyHandler reply: @escaping @Sendable (FSUpgradeItemResult?, (any Error)?) -> Void) {
        do {
            try exact(item); try file.authorize(context); try file.cacheUpgrade()
            reply(FSUpgradeItemResult(grantedCoherency: .noCache), nil)
        } catch { reply(nil, error) }
    }

    func close(_ item: FSItem, context: FSContext, replyHandler reply: @escaping @Sendable () -> Void) {
        if item !== fileItem { reply(); return } // An unrelated object cannot poison or close this item.
        do { try file.authorize(context); file.closed() }
        catch { file.closed(error) } // Void acknowledgement cannot attest successful writeback.
        reply()
    }

    func read(from item: FSItem, at offset: off_t, length: Int, into buffer: FSMutableFileDataBuffer,
              replyHandler reply: @escaping @Sendable (FSReadFileResult?, (any Error)?) -> Void) {
        do { try exact(item); reply(try file.read(at: offset, count: length, buffer: buffer), nil) }
        catch { reply(nil, error) }
    }

    func write(contents: Data, to item: FSItem, at offset: off_t,
               replyHandler reply: @escaping @Sendable (FSWriteFileResult?, (any Error)?) -> Void) {
        do { try exact(item); reply(try file.write(contents, at: offset), nil) }
        catch { reply(nil, error) }
    }

    // These are FSVolume.Handler signatures. They become dispatchable only with the
    // original registered namespace/volume implementation, not by exposing this file alone.
    func getAttributes(_ request: FSItem.GetAttributesRequest, of item: FSItem, context: FSContext,
                       replyHandler reply: @escaping @Sendable (FSGetAttributesResult?, (any Error)?) -> Void) {
        do {
            try exact(item); try file.authorize(context)
            guard let result = FSGetAttributesResult(attributes: try file.fileAttributes(request)) else {
                throw workError(EIO)
            }
            reply(result, nil)
        } catch { reply(nil, error) }
    }

    func setAttributes(_ request: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext,
                       replyHandler reply: @escaping @Sendable (FSSetAttributesResult?, (any Error)?) -> Void) {
        request.consumedAttributes = [] // Unset means "all"; never acknowledge metadata we didn't apply.
        do {
            try exact(item); try file.authorize(context)
            let unsupported: [FSItem.Attribute] = [.fileID, .parentID, .type, .mode, .linkCount, .uid, .gid,
                .flags, .allocSize, .accessTime, .modifyTime, .changeTime, .birthTime, .backupTime, .addedTime,
                .supportsLimitedXAttrs, .inhibitKernelOffloadedIO]
            guard request.isValid(.size), !unsupported.contains(where: { request.isValid($0) })
            else { throw workError(ENOTSUP) }
            // Only size is ever consumed. Unknown future bits also remain unacknowledged.
            let result = try file.resize(request.size)
            request.consumedAttributes = .size
            reply(result, nil)
        } catch { reply(nil, error) }
    }

    func synchronize(flags: FSSyncFlags, replyHandler reply: @escaping @Sendable ((any Error)?) -> Void) {
        do { try file.synchronize(); reply(nil) }
        catch { reply(error) }
    }

    func stopNewFileOpens() { file.stopNewOpens() }
    func finishLocalFileAfterDrain() throws { try file.closeLocalWriterAfterDrain() }
}
