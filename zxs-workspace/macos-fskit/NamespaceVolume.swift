import Darwin
import Foundation
import FSKit

@available(macOS 27.0, *)
extension RemoteDataVolume: FSVolume.Handler {
    var maximumLinkCount: Int { 1 }
    var maximumNameLength: Int { 255 }
    var maximumFileSize: UInt64 { UInt64(WorkFile.maxFile) }
    var maximumXattrSize: Int { 0 }
    var restrictsOwnershipChanges: Bool { true }
    var truncatesLongNames: Bool { false }
    var enableOpenUnlinkEmulation: Bool { false }
    var supportedRenameFlags: FSVolume.RenameFlags { [] }

    var supportedVolumeCapabilities: FSVolume.SupportedCapabilities {
        let supported = FSVolume.SupportedCapabilities()
        supported.caseFormat = names.caseFormat
        supported.doesNotSupportSettingFilePermissions = true
        supported.doesNotSupportImmutableFiles = true
        supported.supportsHardLinks = false; supported.supportsSymbolicLinks = false
        supported.supportsPersistentObjectIDs = false
        return supported
    }

    var volumeStatistics: FSStatFSResult {
        let out = FSStatFSResult(fileSystemTypeName: "zxswork")
        out.blockSize = 4096; out.ioSize = 4096
        // No new files in this namespace. Space means actual LOCAL APFS backing,
        // not available bytes on the server. A property cannot return an errno.
        if (try? visibility()) != nil, let s = try? names.stats(), s.f_bsize > 0 {
            out.blockSize = Int(s.f_bsize); out.ioSize = Int(s.f_iosize)
            out.totalBlocks = UInt64(s.f_blocks); out.freeBlocks = UInt64(s.f_bfree)
            out.availableBlocks = UInt64(s.f_bavail)
            out.usedBlocks = out.totalBlocks >= out.freeBlocks ? out.totalBlocks - out.freeBlocks : 0
            out.totalFiles = UInt64(names.ordered.count + 1); out.freeFiles = 0
        }
        return out
    }

    func rootAttributes(_ request: FSItem.GetAttributesRequest) throws -> FSItem.Attributes {
        guard request.wantedAttributes.subtracting(WorkFile.provided).isEmpty else { throw workError(ENOTSUP) }
        rootAttributesLock.lock(); defer { rootAttributesLock.unlock() }
        let s = try names.rootStat(), a = FSItem.Attributes()
        a.fileID = .rootDirectory; a.parentID = .parentOfRoot; a.type = .directory
        a.uid = getuid(); a.gid = getgid(); a.mode = 0o500; a.linkCount = 2
        a.size = UInt64(max(0, s.st_size)); a.allocSize = UInt64(max(0, s.st_blocks)) * 512
        a.flags = 0; a.accessTime = s.st_atimespec; a.modifyTime = s.st_mtimespec
        a.changeTime = s.st_ctimespec; a.birthTime = s.st_birthtimespec
        a.supportsLimitedXAttrs = false; a.inhibitKernelOffloadedIO = true
        return a
    }

    func getAttributes(_ request: FSItem.GetAttributesRequest, of item: FSItem, context: FSContext,
                       replyHandler reply: @escaping @Sendable (FSGetAttributesResult?, (any Error)?) -> Void) {
        serve(rootMetadata: item === rootItem, reply) {
            try WorkFile.authorize(context)
            let a: FSItem.Attributes
            guard FSGetAttributesResult.requestedAttributes.wantedAttributes.subtracting(WorkFile.provided).isEmpty
            else { throw workError(ENOTSUP) }
            if item === rootItem {
                a = try rootAttributes(request)
            } else { a = try names.row(item).source.file.fileAttributes(request) }
            guard let result = FSGetAttributesResult(attributes: a) else { throw workError(EIO) }
            return result
        }
    }

    func lookupItem(named name: FSFileName, in directory: FSItem, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSLookupItemResult?, (any Error)?) -> Void) {
        // Apple requires serializing return of an FSItem to the kernel with tryReclaim.
        // Dedicated to lookup/reclaim; never take the volume-state lock across a reply.
        lookupAndReclaim.lock(); defer { lookupAndReclaim.unlock() }
        serve(reply) {
            try WorkFile.authorize(context)
            guard directory === rootItem else { throw workError(ENOTDIR) }
            if name.data == Data(".".utf8) || name.data == Data("..".utf8) {
                guard let r = FSLookupItemResult(foundItem: rootItem, itemName: name,
                               itemAttributes: try rootAttributes(FSLookupItemResult.requestedAttributes))
                else { throw workError(EIO) }
                return r
            }
            let row = try names.resolve(name.data)
            let a = try row.source.file.fileAttributes(FSLookupItemResult.requestedAttributes)
            guard let r = FSLookupItemResult(foundItem: row.item, itemName: row.name, itemAttributes: a)
            else { throw workError(EIO) }
            return r // Vendor lookup may be case/normalization-insensitive; reply with exact native spelling.
        }
    }

    func enumerateDirectory(_ directory: FSItem, startingAt cookie: FSDirectoryCookie,
                            verifier: FSDirectoryVerifier, attributes: FSItem.GetAttributesRequest?,
                            packer: FSDirectoryEntryPacker, context: FSContext,
                            replyHandler reply: @escaping @Sendable (FSEnumerateDirectoryResult?, (any Error)?) -> Void) {
        serve(reply) {
            try WorkFile.authorize(context)
            guard directory === rootItem else { throw workError(ENOTDIR) }
            try names.check()
            let dots = attributes == nil ? 2 : 0, total = dots + names.ordered.count
            let version = names.verifier | (dots == 0 ? 1 : 0)
            let invalid = NSError(domain: NSPOSIXErrorDomain, code: Int(FSError.Code.invalidDirectoryCookie.rawValue))
            let start: Int
            if cookie == .initial {
                guard verifier == .initial else { throw invalid }
                start = 0
            } else {
                guard verifier.rawValue == version, (1...UInt64(total)).contains(cookie.rawValue) else { throw invalid }
                start = Int(cookie.rawValue)
            }
            for pos in start..<total {
                let accepted: Bool, next = FSDirectoryCookie(UInt64(pos + 1))
                guard next != .initial else { throw invalid }
                if pos < dots {
                    let label = FSFileName(data: Data((pos == 0 ? "." : "..").utf8))
                    accepted = packer.packEntry(name: label, itemType: .directory, itemID: .rootDirectory,
                                                nextCookie: next, attributes: nil)
                } else {
                    let row = names.ordered[pos-dots]
                    guard try names.row(row.item) === row else { throw workError(ESTALE) }
                    let a = try attributes.map { try row.source.file.fileAttributes($0) }
                    accepted = packer.packEntry(name: row.name, itemType: .file, itemID: row.id,
                                                nextCookie: next, attributes: a)
                }
                if !accepted { break } // FSKit retains the last *accepted* nextCookie for the next page.
            }
            guard let result = FSEnumerateDirectoryResult(verifier: version) else { throw workError(EIO) }
            return result
        }
    }

    // Refuse every namespace mutation. Finder's temp-replace cannot be reported
    // as a successful original write and never issues a remote create/rename/delete.
    func createItem(named name: FSFileName, type: FSItem.ItemType, in directory: FSItem,
                    attributes: FSItem.SetAttributesRequest, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSCreateItemResult?, (any Error)?) -> Void) {
        attributes.consumedAttributes = []; reply(nil, workError(EROFS))
    }
    func removeItem(_ item: FSItem, named name: FSFileName, from directory: FSItem, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSRemoveItemResult?, (any Error)?) -> Void) {
        reply(nil, workError(EROFS))
    }
    func renameItem(_ item: FSItem, inDirectory from: FSItem, named old: FSFileName, to name: FSFileName,
                    inDirectory to: FSItem, overItem target: FSItem?, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSRenameItemResult?, (any Error)?) -> Void) {
        reply(nil, workError(EROFS))
    }
    func renameItem(_ item: FSItem, inDirectory from: FSItem, named old: FSFileName, to name: FSFileName,
                    inDirectory to: FSItem, overItem target: FSItem?, flags: FSVolume.RenameFlags, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSRenameItemResult?, (any Error)?) -> Void) {
        reply(nil, workError(EROFS))
    }
    func createLink(to item: FSItem, named name: FSFileName, in directory: FSItem, context: FSContext,
                    replyHandler reply: @escaping @Sendable (FSCreateLinkResult?, (any Error)?) -> Void) {
        reply(nil, workError(ENOTSUP))
    }
    func createSymbolicLink(named name: FSFileName, in directory: FSItem, attributes: FSItem.SetAttributesRequest,
                            linkContents: FSFileName, context: FSContext,
                            replyHandler reply: @escaping @Sendable (FSCreateSymlinkResult?, (any Error)?) -> Void) {
        attributes.consumedAttributes = []; reply(nil, workError(ENOTSUP))
    }
    func readSymbolicLink(_ item: FSItem, context: FSContext,
                          replyHandler reply: @escaping @Sendable (FSReadSymlinkResult?, (any Error)?) -> Void) {
        reply(nil, workError(ENOTSUP))
    }
}
