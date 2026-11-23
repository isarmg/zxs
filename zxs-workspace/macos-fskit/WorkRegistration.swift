import CryptoKit
import Darwin
import Foundation
import FSKit

// All text in the registration is ASCII. Remote UTF-8 is hex, never re-normalized by JSON.
@available(macOS 27.0, *)
struct WorkProjection: Codable {
    struct Entry: Codable {
        let container, remoteNameHex, branch, head, node, parent: String
        let revision: UInt64
        let body: WorkWitness, generation: WorkGeneration
    }
    let format, token, volume, destination, account: String
    let directory, emptyTicket, names, record: WorkWitness
    let entries: [Entry]

    static func hex(_ s: String, _ size: Int, id: Bool = false) -> Bool {
        s.utf8.count == size && (!id || s.contains(where: { $0 != "0" })) &&
            s.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    static func name(_ h: String) throws -> Data {
        let n = h.utf8.count
        guard n >= 2, n <= 510, n % 2 == 0, hex(h,n) else { throw workError(EINVAL) }
        let u = Array(h.utf8)
        func nibble(_ v: UInt8) -> UInt8 { v <= 57 ? v-48 : v-87 }
        let b = Data(stride(from: 0, to: n, by: 2).map { nibble(u[$0]) << 4 | nibble(u[$0+1]) })
        guard !b.contains(0), !b.contains(47), b != Data(".".utf8), b != Data("..".utf8),
              let text = String(data: b, encoding: .utf8), Data(text.utf8) == b else { throw workError(EINVAL) }
        return b
    }
    static func parse(_ raw: Data, token wanted: String) throws -> Self {
        guard !raw.isEmpty, raw.count <= 1 << 20 else { throw workError(EFBIG) }
        let r = try JSONDecoder().decode(Self.self, from: raw)
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys,.withoutEscapingSlashes]
        func alnum(_ c: UInt8) -> Bool { (48...57).contains(c) || (97...122).contains(c) }
        guard try encoder.encode(r) == raw, r.format == "zxs-fskit-projection-1",
              hex(wanted,32,id:true), r.token == wanted, let v = UUID(uuidString:r.volume),
              v.uuidString.lowercased() == r.volume, r.volume != "00000000-0000-0000-0000-000000000000",
              hex(r.destination,64), let a = r.account.utf8.first, alnum(a), r.account.utf8.count <= 64,
              r.account.utf8.allSatisfy({ alnum($0) || [45,46,95].contains($0) }),
              (1...128).contains(r.entries.count) else { throw workError(EINVAL) }
        let witnesses = [r.directory,r.emptyTicket,r.names,r.record] + r.entries.map(\.body)
        var objects = Set<UInt64>(), previous = ""
        for w in witnesses {
            guard hex(w.volumeHex,32,id:true), w.volumeHex == r.directory.volumeHex, w.inode > 0,
                  w.birthSec > 0, (0..<1000000000).contains(w.birthNsec), objects.insert(w.inode).inserted
            else { throw workError(ESTALE) }
        }
        for e in r.entries {
            let g = e.generation
            guard hex(e.container,32,id:true), previous < e.container, e.revision > 0,
                  e.branch == "main" || hex(e.branch,32,id:true), hex(e.head,32,id:true), hex(e.node,32,id:true),
                  e.head == e.node || e.branch == "main", e.parent.isEmpty || hex(e.parent,32,id:true), e.parent != e.node,
                  g.size >= 0, g.size <= maximumWorkFileBytes, hex(g.sha256,64),
                  (0..<1000000000).contains(g.mtimeNsec), (0..<1000000000).contains(g.ctimeNsec)
            else { throw workError(EINVAL) }
            _ = try name(e.remoteNameHex); previous = e.container
        }
        return r
    }
}

@available(macOS 27.0, *)
final class RegisteredWork {
    let ticket: HeldMountTicket, plan: WorkProjection
    private let group: FileHandle, record: FileHandle, stamp: stat, base: URL
    private static let recordName = "projection.json"
    private init(group: FileHandle, base: URL, ticket: HeldMountTicket, plan: WorkProjection,
                 record: FileHandle, stamp: stat) {
        self.group=group; self.base=base; self.ticket=ticket; self.plan=plan; self.record=record; self.stamp=stamp
    }

    static func groupURL() throws -> URL {
        guard let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: HeldMountTicket.group),
              base.isFileURL else { throw workError(EPERM) }
        return base
    }
    static func groupHandle(_ base: URL) throws -> FileHandle {
        let fd = base.withUnsafeFileSystemRepresentation { p -> Int32 in
            guard let p else { return -1 }; return zxs_open_group_dir(p)
        }
        guard fd >= 0 else { throw workError(errno) }
        return FileHandle(fileDescriptor:fd,closeOnDealloc:true)
    }
    private static func literal(_ lhs: URL, _ rhs: URL) -> Bool {
        guard lhs.isFileURL, lhs.host == nil || lhs.host == "", lhs.user == nil, lhs.password == nil,
              lhs.port == nil, lhs.query == nil, lhs.fragment == nil else { return false }
        return lhs.withUnsafeFileSystemRepresentation { a in rhs.withUnsafeFileSystemRepresentation { b in
            guard let a, let b else { return false }; return strcmp(a,b)==0
        } }
    }
    // Examine only URL bytes. Never inspect an unrelated file URL, grant access to
    // arbitrary paths, enumerate the group, or interpret visible remote names as local paths.
    static func token(for resource: FSResource) throws -> String? {
        guard let r = resource as? FSPathURLResource, !r.isWritable, !r.isRevoked else { return nil }
        let leaf=r.url.deletingLastPathComponent().lastPathComponent
        guard leaf.hasPrefix("work-") else { return nil }
        let token=String(leaf.dropFirst(5))
        guard WorkProjection.hex(token,32,id:true) else { return nil }
        try HostSigning.checkRunningModule()
        let base=try groupURL()
        guard literal(r.url,base.appendingPathComponent("work-"+token,isDirectory:true).appendingPathComponent("ticket"))
        else { return nil }
        return token
    }
    private static func statAt(_ parent: FileHandle, _ n: String) throws -> stat {
        var s=stat()
        guard fstatat(parent.fileDescriptor,n,&s,AT_SYMLINK_NOFOLLOW)==0 else { throw workError(errno) }
        return s
    }
    private static func same(_ a: stat, _ b: stat) -> Bool {
        a.st_dev==b.st_dev && a.st_ino==b.st_ino && a.st_birthtimespec.tv_sec==b.st_birthtimespec.tv_sec &&
            a.st_birthtimespec.tv_nsec==b.st_birthtimespec.tv_nsec
    }
    private static func sameRecord(_ a: stat,_ b: stat) -> Bool {
        same(a,b) && b.st_size==a.st_size && b.st_ctimespec.tv_sec==a.st_ctimespec.tv_sec &&
            b.st_ctimespec.tv_nsec==a.st_ctimespec.tv_nsec && b.st_mtimespec.tv_sec==a.st_mtimespec.tv_sec &&
            b.st_mtimespec.tv_nsec==a.st_mtimespec.tv_nsec && b.st_nlink==1 && b.st_flags==0 &&
            (b.st_mode & 0o177777)==0o100400 && b.st_uid==getuid()
    }
    private static func child(_ parent: FileHandle,_ n: String,_ directory: Bool) throws -> FileHandle {
        let s=try statAt(parent,n), fd=n.withCString {
            zxs_open_registration_leaf(parent.fileDescriptor,$0,directory ? 1 : 0,s.st_ino,
                                      Int64(s.st_birthtimespec.tv_sec),Int64(s.st_birthtimespec.tv_nsec))
        }
        guard fd>=0 else { throw workError(errno) }
        let h=FileHandle(fileDescriptor:fd,closeOnDealloc:true); var actual=stat()
        guard fstat(fd,&actual)==0, same(s,actual), same(try statAt(parent,n),actual),
              n.withCString({ zxs_work_spelling(fd,$0) })==0 else { throw workError(ESTALE) }
        return h
    }
    private static func load(token: String, expected: String?) throws -> RegisteredWork {
        guard WorkProjection.hex(token,32,id:true), expected.map({ WorkProjection.hex($0,64) }) ?? true
        else { throw workError(EINVAL) }
        let base=try groupURL(), group=try groupHandle(base), dir=try child(group,"work-"+token,true)
        let empty=try child(dir,"ticket",false), s=try statAt(dir,recordName)
        guard s.st_size>0, s.st_size<=1<<20, sameRecord(s,s) else { throw workError(EINVAL) }
        let fd = recordName.withCString { zxs_open_marker(dir.fileDescriptor,$0,s.st_ino,
                                   Int64(s.st_birthtimespec.tv_sec),Int64(s.st_birthtimespec.tv_nsec)) }
        guard fd>=0 else { throw workError(errno) }
        let record=FileHandle(fileDescriptor:fd,closeOnDealloc:true)
        var b=[UInt8](repeating:0,count:Int(s.st_size)), at=0
        while at<b.count {
            let n=b.withUnsafeMutableBytes { pread(fd,$0.baseAddress!.advanced(by:at),$0.count-at,off_t(at)) }
            if n<0 && errno==EINTR { continue }
            guard n>0 else { throw workError(n==0 ? EIO : errno) }; at+=n
        }
        let raw=Data(b), digest=SHA256.hash(data:raw).map { String(format:"%02x",$0) }.joined()
        guard expected == nil || expected==digest else { throw workError(ESTALE) }
        let p=try WorkProjection.parse(raw,token:token)
        guard let v=UUID(uuidString:p.volume), p.record.inode==s.st_ino,
              p.record.birthSec==Int64(s.st_birthtimespec.tv_sec), p.record.birthNsec==Int64(s.st_birthtimespec.tv_nsec)
        else { throw workError(ESTALE) }
        let ticket=try HeldMountTicket(token:token,digest:digest,expectedVolume:v,directory:dir,
                                      registered:p.directory,emptyTicket:empty,issued:p.emptyTicket)
        let out=RegisteredWork(group:group,base:base,ticket:ticket,plan:p,record:record,stamp:s)
        try out.check(); return out
    }
    #if !ZXS_FSKIT_PROVIDER
    static func issued(token: String, digest: String) throws -> RegisteredWork {
        // Host caller supplies the digest of its own completed selection, never lower trust URL material.
        try HostSigning.checkHostProcess()
        return try load(token:token,expected:digest)
    }
    #endif
    #if !ZXS_FSKIT_HOST
    static func attempted(token: String) throws -> RegisteredWork {
        try HostSigning.checkRunningModule()
        let r=try load(token:token,expected:nil)
        guard try r.ticket.marked(.attempted), try !r.ticket.marked(.loaded),
              try !r.ticket.marked(.verified), try !r.ticket.marked(.stop) else { throw workError(ESTALE) }
        return r // Original host-attempt embeds the digest and original physical ticket, not just the URL.
    }

    #endif

    func check() throws {
        try ticket.check()
        var g=stat(), d=stat(), r=stat()
        let ok=base.withUnsafeFileSystemRepresentation { p in p==nil ? EINVAL : zxs_work_path(group.fileDescriptor,p!) }
        guard ok==0, fstat(group.fileDescriptor,&g)==0, (g.st_mode & 0o022)==0,
              g.st_uid==getuid(), g.st_flags==0, fstat(ticket.directory.fileDescriptor,&d)==0,
              Self.same(d,try Self.statAt(group,"work-"+ticket.token)), d.st_dev==g.st_dev,
              fstat(record.fileDescriptor,&r)==0, r.st_dev==d.st_dev, Self.sameRecord(stamp,r),
              Self.sameRecord(stamp,try Self.statAt(ticket.directory,Self.recordName)),
              Self.recordName.withCString({ zxs_work_spelling(record.fileDescriptor,$0) })==0
        else { throw workError(ESTALE) }
    }
    #if !ZXS_FSKIT_PROVIDER
    var scopeSHA:String {
        let parts=[ticket.token,ticket.digest,plan.destination,plan.account] + plan.entries.map {
            $0.container+":"+$0.branch+":"+$0.node
        }
        return SHA256.hash(data:Data((parts.joined(separator:"\n")+"\n").utf8)).map { String(format:"%02x",$0) }.joined()
    }
    func drained() throws -> DrainedFiles {
        try check()
        guard let raw=try ticket.witness(.stopped) else { throw workError(EBUSY) }
        return try DrainedFiles.decode(raw,registration:self)
    }
    struct Loan:Codable {
        let token,digest,destination,account:String
        let directory,emptyTicket:WorkWitness
        let source:WorkProjection.Entry, final:DrainedFiles.Row
    }
    func loan(_ cid:String) throws -> (FileHandle,FileHandle,FileHandle,Data) {
        let complete=try drained()
        guard let i=complete.entries.firstIndex(where: { $0.container==cid }) else { throw workError(ENOENT) }
        let row=complete.entries[i], src=plan.entries[i], name="body-"+cid
        let fd=name.withCString { zxs_open_work_leaf(ticket.directory.fileDescriptor,$0,O_RDONLY,row.body.inode,
                                                     row.body.birthSec,row.body.birthNsec) }
        guard fd>=0 else { throw workError(errno) }
        let held=FileHandle(fileDescriptor:fd,closeOnDealloc:true)
        try row.body.check(fd,directory:false)
        let flags=fcntl(fd,F_GETFL); var s=stat()
        guard flags>=0, flags&O_ACCMODE==O_RDONLY, flags&O_APPEND==0,
              flock(fd,LOCK_SH|LOCK_NB)==0 else { throw workError(errno==0 ? ESTALE : errno) }
        guard fstat(fd,&s)==0, row.generation.matches(s), Self.same(s,try Self.statAt(ticket.directory,name)),
              name.withCString({ zxs_work_spelling(fd,$0) })==0 else { throw workError(ESTALE) }
        let proof=Loan(token:ticket.token,digest:ticket.digest,destination:plan.destination,account:plan.account,
                       directory:plan.directory,emptyTicket:plan.emptyTicket,source:src,final:row)
        let e=JSONEncoder(); e.outputFormatting=[.sortedKeys,.withoutEscapingSlashes]
        let raw=try e.encode(proof)
        try check()
        return (held,ticket.directory,ticket.ticket,raw)
    }
    #endif

    // Module only, after the exclusive cross-process provider-loaded marker. Probe never opens
    // any body or creates namespace entries. Prior originals never reconstructed from DISPLAY names.
    #if !ZXS_FSKIT_HOST
    func loadNames() throws -> NameIndex {
        func authorized() throws {
            try check()
            guard try ticket.marked(.attempted), try ticket.marked(.loaded), try !ticket.marked(.verified),
                  try !ticket.marked(.stop) else { throw workError(ESTALE) }
        }
        try authorized()
        let names=try Self.child(ticket.directory,"names",true)
        try plan.names.check(names.fileDescriptor,directory:true)
        var selections=[SelectedWorkItem]()
        for e in plan.entries {
            try authorized()
            let f=try WorkFile(parent:ticket.directory,parentID:plan.directory,fileID:e.body,
                               generation:e.generation,container:e.container)
            selections.append(SelectedWorkItem(containerID:e.container,remoteName:try WorkProjection.name(e.remoteNameHex),file:f))
        }
        let index=try NameIndex(directory:names,registered:plan.names,selected:selections)
        try index.checkOwner(ticket); try authorized(); return index
    }
    #endif
}

/// Explicit local write generation, not a remote receipt. Computed after all original
/// writers are closed, before host is allowed even to ask the kernel for normal detach.
@available(macOS 27.0, *)
struct DrainedFiles: Codable {
    struct Row: Codable { let container:String, body:WorkWitness, generation:WorkGeneration }
    let format, token, digest:String
    let entries:[Row]

    func encoded() throws -> Data {
        let e=JSONEncoder(); e.outputFormatting=[.sortedKeys,.withoutEscapingSlashes]
        let raw=try e.encode(self)
        guard raw.count>0, raw.count<=256<<10 else { throw workError(EFBIG) }
        return raw
    }
    static func decode(_ raw:Data,registration:RegisteredWork) throws -> Self {
        guard !raw.isEmpty, raw.count<=256<<10 else { throw workError(EINVAL) }
        let all=try JSONDecoder().decode(Self.self,from:raw)
        guard try all.encoded()==raw, all.format=="zxs-fskit-stopped-1", all.token==registration.ticket.token,
              all.digest==registration.ticket.digest, all.entries.count==registration.plan.entries.count
        else { throw workError(ESTALE) }
        for (f,src) in zip(all.entries,registration.plan.entries) {
            let g=f.generation
            guard f.container==src.container, f.body==src.body, g.size>=0, g.size<=maximumWorkFileBytes,
                  WorkProjection.hex(g.sha256,64), (0..<1000000000).contains(g.mtimeNsec),
                  (0..<1000000000).contains(g.ctimeNsec) else { throw workError(ESTALE) }
        }
        return all
    }
}
