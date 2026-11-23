import Darwin
import Foundation
import FSKit

/// Inert source assertions. No SDK compile, signed host, disk, fixture, FSClient or target is run here.
@available(macOS 27.0, *)
enum FSKitProjectionQualification {
    static func canonicalRegistration(_ raw:Data,token:String,expectedRawName:Data) throws {
        let r=try WorkProjection.parse(raw,token:token)
        guard r.entries.count==1, try WorkProjection.name(r.entries[0].remoteNameHex)==expectedRawName
        else { throw workError(EIO) }
        let alternatives=[Data([32])+raw, Data("{\"not-a-field\":1,".utf8)+Data(raw.dropFirst()),
                          Data("{\"token\":\"00000000000000000000000000000000\",".utf8)+Data(raw.dropFirst())]
        for replacement in alternatives {
            do { _=try WorkProjection.parse(replacement,token:token) } catch { continue }
            throw workError(EIO)
        }
    }
    static func wrongHostDigest(_ token:String,originalDigest:String) throws {
        let wrong=(originalDigest.first=="a" ? "b" : "a")+String(originalDigest.dropFirst())
        do { _=try RegisteredWork.issued(token:token,digest:wrong) }
        catch let e as NSError where e.domain==NSPOSIXErrorDomain && e.code==Int(ESTALE) { return }
        throw workError(EIO)
    }
}
