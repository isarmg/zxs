import Darwin
import Foundation

// Linked into the optional signed macOS 27 desktop process. The ordinary ad-hoc app
// has no symbols. No dylib path, user-selected group, CLI, helper process or new service.
@available(macOS 27.0, *)
@_cdecl("zxs_fskit_group_fd")
public func zxsDesktopGroupFD() -> Int32 {
    do {
        try HostSigning.checkHostProcess()
        let h=try RegisteredWork.groupHandle(RegisteredWork.groupURL())
        let copy=fcntl(h.fileDescriptor,F_DUPFD_CLOEXEC,0)
        guard copy>=0 else { throw workError(errno) }
        return copy // Caller owns exactly this FD. No chosen folder is ever authorized.
    } catch { return -abiErr(error) }
}
private func abiErr(_ e: any Error) -> Int32 {
    let n=e as NSError
    if n.domain==NSPOSIXErrorDomain, let code=Int32(exactly:n.code), code>0 { return code }; return EIO
}

@available(macOS 27.0, *)
@MainActor
private final class DesktopSession {
    static var sessions=[UInt64:DesktopSession](), next:UInt64=1
    let host: OriginalMountHost
    var mounting=false, detaching=false, lastErr:Int32=0, returned:URL?
    init(_ registration: RegisteredWork) { host=OriginalMountHost(registration:registration) }
    // The first request only stages an async OS call; never claim a mount before verification.
    func action(_ verb:Int32) throws {
        guard !mounting, !detaching else { throw workError(EBUSY) }
        switch verb {
        case 1:
            guard host.state == .ready else { throw workError(EBUSY) }
            mounting=true; lastErr=0
            Task { @MainActor in
                defer { self.mounting=false }
                do { self.returned=try await self.host.attach(confirmed:true) }
                catch { self.lastErr=abiErr(error) }
            }
        case 2: try host.requestStop(confirmed:true); lastErr=0
        case 3:
            guard host.state == .stopping else { throw workError(EBUSY) }
            detaching=true; lastErr=0
            Task { @MainActor in
                defer { self.detaching=false }
                do { try await self.host.normalUnmount(confirmed:true) }
                catch { self.lastErr=abiErr(error) }
            }
        default: throw workError(EINVAL)
        }
    }
    func status() throws -> Int32 {
        if mounting { return 2 }; if detaching { return 8 }
        if host.state == .mounted || host.state == .stopping {
            do { try host.checkStillMounted() } catch { lastErr=abiErr(error); return 3 }
        }
        switch host.state {
        case .ready: return 1
        case .checking: return 2
        case .uncertain: return 3
        case .mounted: return 4
        case .stopping: return 5
        case .normalGone: return try host.confirmedLocalTeardown() ? 7 : 6
        }
    }
}

private func ascii(_ p:UnsafePointer<CChar>?, _ n:Int) throws -> String {
    guard let p, strnlen(p,n+1)==n else { throw workError(EINVAL) }
    let bytes=UnsafeBufferPointer(start:UnsafeRawPointer(p).assumingMemoryBound(to:UInt8.self),count:n)
    guard bytes.allSatisfy({ $0<128 }) else { throw workError(EINVAL) }
    return String(decoding:bytes,as:UTF8.self)
}

/// One asynchronous installed/enabled+signing check per requested checkout before its first
/// authenticated GET. The later attach rechecks it; this receipt isn't mount or write authority.
@available(macOS 27.0, *)
private final class InstalledGate: @unchecked Sendable {
    static let shared = InstalledGate()
    private let lock=NSLock()
    private var results=[String:(UUID,Int32)]()
    func start(_ token:String) throws -> UUID {
        lock.lock(); defer { lock.unlock() }
        guard results[token]==nil, results.count<128 else { throw workError(EBUSY) }
        let stamp=UUID(); results[token]=(stamp,0); return stamp
    }
    func record(_ token:String,_ stamp:UUID,_ result:Int32) {
        lock.lock(); defer { lock.unlock() }
        if results[token]?.0==stamp { results[token]=(stamp,result) }
    }
    func cancel(_ token:String) { lock.lock(); results.removeValue(forKey:token); lock.unlock() }
    func status(_ token:String,consume:Bool) -> Int32 {
        lock.lock(); defer { lock.unlock() }
        guard let (_,value)=results[token] else { return -ESTALE }
        if consume && value != 0 { results.removeValue(forKey:token) }
        return value
    }
}
@available(macOS 27.0, *)
@_cdecl("zxs_fskit_preflight")
public func zxsDesktopPreflight(_ input:UnsafePointer<CChar>?,_ verb:Int32) -> Int32 {
    do {
        let token=try ascii(input,32)
        guard WorkProjection.hex(token,32,id:true) else { throw workError(EINVAL) }
        switch verb {
        case 0: return InstalledGate.shared.status(token,consume:false)
        case 1:
            guard Thread.isMainThread else { throw workError(EINVAL) }
            let stamp=try InstalledGate.shared.start(token)
            Task { @MainActor in
                do { try await HostSigning.checkInstalled(); InstalledGate.shared.record(token,stamp,1) }
                catch { InstalledGate.shared.record(token,stamp,-abiErr(error)) }
            }
            return 0
        case 2: return InstalledGate.shared.status(token,consume:true)
        case 3: InstalledGate.shared.cancel(token); return 0
        default: throw workError(EINVAL)
        }
    } catch { return -abiErr(error) }
}

@available(macOS 27.0, *)
@_cdecl("zxs_fskit_session")
public func zxsDesktopSession(_ token:UnsafePointer<CChar>?,_ digest:UnsafePointer<CChar>?,
                       _ out:UnsafeMutablePointer<UInt64>?) -> Int32 {
    guard Thread.isMainThread, let out else { return EINVAL }
    do {
        let t=try ascii(token,32), d=try ascii(digest,64)
        let key=try MainActor.assumeIsolated {
            guard DesktopSession.sessions.count<128, DesktopSession.next != UInt64.max else { throw workError(EOVERFLOW) }
            let original=try RegisteredWork.issued(token:t,digest:d)
            let key=DesktopSession.next; DesktopSession.next+=1
            DesktopSession.sessions[key]=DesktopSession(original); return key
        }
        out.pointee=key; return 0
    } catch { return abiErr(error) }
}

@available(macOS 27.0, *)
@_cdecl("zxs_fskit_action")
public func zxsDesktopAction(_ key:UInt64,_ verb:Int32) -> Int32 {
    guard Thread.isMainThread else { return EINVAL }
    do {
        try MainActor.assumeIsolated {
            guard let s=DesktopSession.sessions[key] else { throw workError(ESTALE) }
            do { try s.action(verb) } catch { s.lastErr=abiErr(error); throw error }
        }; return 0
    } catch { return abiErr(error) }
}

@available(macOS 27.0, *)
@_cdecl("zxs_fskit_status")
public func zxsDesktopStatus(_ key:UInt64,_ state:UnsafeMutablePointer<Int32>?,_ lastError:UnsafeMutablePointer<Int32>?,
                      _ path:UnsafeMutablePointer<CChar>?,_ capacity:Int) -> Int32 {
    guard Thread.isMainThread, let state, let lastError, let path, capacity>0 else { return EINVAL }
    do {
        let (phase,error,bytes)=try MainActor.assumeIsolated { () throws -> (Int32,Int32,[UInt8]) in
            guard let s=DesktopSession.sessions[key] else { throw workError(ESTALE) }
            let phase=try s.status()
            if phase==4 || phase==5 {
                guard let url=s.returned else { throw workError(ESTALE) }
                let b=try url.withUnsafeFileSystemRepresentation { p -> [UInt8] in
                    guard let p else { throw workError(EINVAL) }
                    return Array(UnsafeBufferPointer(start:UnsafeRawPointer(p).assumingMemoryBound(to:UInt8.self),
                                                     count:strlen(p)))
                }; return (phase,s.lastErr,b)
            }
            return (phase,s.lastErr,[])
        }
        guard bytes.count<capacity else { throw workError(ERANGE) }
        for (i,b) in bytes.enumerated() { path[i]=CChar(bitPattern:b) }
        path[bytes.count]=0; state.pointee=phase; lastError.pointee=error; return 0
    } catch { return abiErr(error) }
}

// Known key only. No FSClient call: handles from a surviving host prevent a
// competing restore. An incomplete prior detach is not eligible even after reboot.
@available(macOS 27.0, *)
@_cdecl("zxs_fskit_completed")
public func zxsDesktopCompleted(_ token:UnsafePointer<CChar>?,_ digest:UnsafePointer<CChar>?,
                                _ scope:UnsafePointer<CChar>?, _ out:UnsafeMutablePointer<UInt64>?) -> Int32 {
    guard Thread.isMainThread, let out else { return EINVAL }
    do {
        let t=try ascii(token,32), d=try ascii(digest,64), expected=try ascii(scope,64)
        let key=try MainActor.assumeIsolated {
            guard DesktopSession.sessions.count<128, DesktopSession.next != UInt64.max else { throw workError(EOVERFLOW) }
            let original=try RegisteredWork.issued(token:t,digest:d)
            guard original.scopeSHA==expected else { throw workError(EPERM) }
            let s=DesktopSession(original)
            try s.host.restoreCompleted()
            let key=DesktopSession.next; DesktopSession.next+=1
            DesktopSession.sessions[key]=s; return key
        }
        out.pointee=key; return 0
    } catch { return abiErr(error) }
}

@available(macOS 27.0, *)
@_cdecl("zxs_fskit_lend")
public func zxsDesktopLend(_ key:UInt64,_ container:UnsafePointer<CChar>?,_ fds:UnsafeMutablePointer<Int32>?,
                           _ output:UnsafeMutablePointer<CChar>?,_ capacity:Int) -> Int32 {
    guard Thread.isMainThread, let fds, let output, capacity>0 else { return EINVAL }
    do {
        let cid=try ascii(container,32)
        let (owned,raw)=try MainActor.assumeIsolated { () throws -> ([Int32],[UInt8]) in
            guard let s=DesktopSession.sessions[key] else { throw workError(ESTALE) }
            let (body,dir,ticket,proof)=try s.host.loan(cid)
            guard proof.count<capacity else { throw workError(ERANGE) }
            var completed=[Int32]()
            for h in [body,dir,ticket] {
                let fd=fcntl(h.fileDescriptor,F_DUPFD_CLOEXEC,0)
                if fd<0 { let e=errno; for prior in completed { _=Darwin.close(prior) }; throw workError(e) }
                completed.append(fd)
            }
            return (completed,Array(proof))
        }
        for i in 0..<3 { fds[i]=owned[i] }
        for (i,b) in raw.enumerated() { output[i]=CChar(bitPattern:b) }; output[raw.count]=0
        return 0 // Three truly owned descriptors; the ticket preserves original lease across a handoff.
    } catch { return abiErr(error) }
}

@available(macOS 27.0, *)
@_cdecl("zxs_fskit_release_completed")
public func zxsDesktopRelease(_ key:UInt64) -> Int32 {
    guard Thread.isMainThread else { return EINVAL }
    do {
        try MainActor.assumeIsolated {
            guard let s=DesktopSession.sessions[key], !s.mounting, !s.detaching else { throw workError(EBUSY) }
            guard try s.host.confirmedLocalTeardown() else { throw workError(EBUSY) }
            DesktopSession.sessions.removeValue(forKey:key)
            // Never remove a file, pending operation or known key. Any lent FD
            // keeps its original OS lease. Cold reopen requires all original proofs.
        }; return 0
    } catch { return abiErr(error) }
}
