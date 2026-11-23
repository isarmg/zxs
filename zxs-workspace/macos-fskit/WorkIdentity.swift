import Darwin
import Foundation

let maximumWorkFileBytes: off_t = 128 << 30

// These values must come from the controller's existing durable, private work
// registration, not from URL input or statting a new selection under a display name.
struct WorkWitness: Codable, Equatable {
    let volumeHex: String
    let inode: UInt64
    let birthSec: Int64
    let birthNsec: Int64

    func check(_ fd: Int32, directory: Bool) throws {
        let code = volumeHex.withCString {
            zxs_work_evidence(fd, directory ? 1 : 0, $0, inode, birthSec, birthNsec)
        }
        if code != 0 { throw workError(code) }
    }
}

func workError(_ value: Int32) -> NSError {
    NSError(domain: NSPOSIXErrorDomain, code: Int(value))
}

struct WorkGeneration: Codable, Equatable {
    let size: Int64
    let mtimeSec: Int64, mtimeNsec: Int64
    let ctimeSec: Int64, ctimeNsec: Int64
    let sha256: String

    func matches(_ s: stat) -> Bool {
        size == s.st_size && mtimeSec == Int64(s.st_mtimespec.tv_sec) && mtimeNsec == Int64(s.st_mtimespec.tv_nsec)
            && ctimeSec == Int64(s.st_ctimespec.tv_sec) && ctimeNsec == Int64(s.st_ctimespec.tv_nsec)
    }
}
