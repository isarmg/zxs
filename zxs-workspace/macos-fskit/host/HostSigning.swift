import Darwin
import Foundation
import FSKit
import Security

/// Read-only preflight. Never install a provider, enable it, open Settings, request
/// privileges, look up a short type name, or run a command as a fallback.
@available(macOS 27.0, *)
enum HostSigning {
    static let hostID = "org.zipworkspace.desktop"
    static let moduleID = "org.zipworkspace.desktop.remotefs"
    static let group = HeldMountTicket.group
    private static let base = SecCSFlags(rawValue: 0)

    private static func requireCode(_ bundle: String, team: String) throws -> SecRequirement {
        // Only a 10-character distribution team from the signed release configuration.
        guard team.utf8.count == 10, team.utf8.allSatisfy({ (65...90).contains($0) || (48...57).contains($0) })
        else { throw workError(EPERM) }
        let spec = "anchor apple generic and identifier \"" + bundle +
            "\" and certificate leaf[subject.OU] = \"" + team + "\""
        var out: SecRequirement?
        guard SecRequirementCreateWithString(spec as CFString, base, &out) == errSecSuccess, let out
        else { throw workError(EPERM) }
        return out
    }

    private static func checkFile(_ url: URL, requirement: SecRequirement) throws -> NSDictionary {
        var code: SecStaticCode?, info: CFDictionary?
        let flags = SecCSFlags(rawValue: kSecCSCheckAllArchitectures | kSecCSStrictValidate | kSecCSNoNetworkAccess)
        guard SecStaticCodeCreateWithPath(url as CFURL, base, &code) == errSecSuccess, let code,
              SecStaticCodeCheckValidity(code, flags, requirement) == errSecSuccess,
              SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
              let result = info as NSDictionary? else { throw workError(EPERM) }
        return result
    }

    private static func yes(_ any: Any?) -> Bool {
        guard let value = any else { return false }
        let r = value as CFTypeRef
        return CFGetTypeID(r) == CFBooleanGetTypeID() && CFEqual(r, kCFBooleanTrue)
    }

    static func checkRunningModule() throws {
        guard getuid() != 0, getuid() == geteuid(), getgid() == getegid(),
              Bundle.main.bundleIdentifier == moduleID,
              let team = Bundle.main.object(forInfoDictionaryKey: "ZxsDistributionTeam") as? String,
              let task = SecTaskCreateFromSelf(nil),
              yes(SecTaskCopyValueForEntitlement(task, "com.apple.developer.fskit.fsmodule" as CFString, nil)),
              yes(SecTaskCopyValueForEntitlement(task, "com.apple.security.app-sandbox" as CFString, nil)),
              !yes(SecTaskCopyValueForEntitlement(task, "com.apple.security.get-task-allow" as CFString, nil)),
              let groups = SecTaskCopyValueForEntitlement(task, "com.apple.security.application-groups" as CFString,nil)
                  as? [String], groups.contains(group) else { throw workError(EPERM) }
        let req = try requireCode(moduleID, team: team)
        var original: SecCode?
        guard SecCodeCopySelf(base,&original) == errSecSuccess, let original,
              SecCodeCheckValidity(original,SecCSFlags(rawValue: kSecCSStrictValidate | kSecCSNoNetworkAccess),
                                   req) == errSecSuccess else { throw workError(EPERM) }
    }

    static func checkInstalled() async throws {
        guard let team = Bundle.main.object(forInfoDictionaryKey: "ZxsDistributionTeam") as? String,
              Bundle.main.bundleIdentifier == hostID, let embedded = Bundle.main.builtInPlugInsURL,
              getuid() != 0, geteuid() == getuid(), getgid() == getegid() else { throw workError(EPERM) }
        let host = try requireCode(hostID, team: team), module = try requireCode(moduleID, team: team)
        var current: SecCode?
        guard SecCodeCopySelf(base, &current) == errSecSuccess, let current,
              SecCodeCheckValidity(current, SecCSFlags(rawValue: kSecCSStrictValidate | kSecCSNoNetworkAccess),
                                   host) == errSecSuccess,
              let task = SecTaskCreateFromSelf(nil),
              yes(SecTaskCopyValueForEntitlement(task, "com.apple.developer.fskit.mount" as CFString, nil)),
              let groups = SecTaskCopyValueForEntitlement(task, "com.apple.security.application-groups" as CFString, nil)
                  as? [String], groups.contains(group),
              !yes(SecTaskCopyValueForEntitlement(task, "com.apple.security.get-task-allow" as CFString, nil))
        else { throw workError(EPERM) }
        let modules = try await FSClient.shared.installedExtensions.filter { $0.bundleIdentifier == moduleID }
        guard modules.count == 1, let installed = modules.first, installed.isEnabled else { throw workError(ENODEV) }
        let expected = embedded.appendingPathComponent("ZxsRemoteFS.appex", isDirectory: true)
        func literal(_ url: URL) throws -> [UInt8] {
            guard url.isFileURL, url.host == nil || url.host == "", url.query == nil, url.fragment == nil
            else { throw workError(ESTALE) }
            return try url.withUnsafeFileSystemRepresentation { p in
                guard let p else { throw workError(EINVAL) }
                return Array(UnsafeBufferPointer(start: UnsafeRawPointer(p).assumingMemoryBound(to: UInt8.self),
                                                 count: strlen(p)))
            }
        }
        guard try literal(installed.url) == literal(expected),
              try literal(expected) == literal(expected.resolvingSymlinksInPath()) else { throw workError(ESTALE) }
        let selfInfo = try checkFile(Bundle.main.bundleURL, requirement: host)
        let extInfo = try checkFile(expected, requirement: module)
        guard selfInfo[kSecCodeInfoTeamIdentifier] as? String == team,
              extInfo[kSecCodeInfoTeamIdentifier] as? String == team,
              let ent = extInfo[kSecCodeInfoEntitlementsDict] as? NSDictionary,
              yes(ent["com.apple.developer.fskit.fsmodule"]), yes(ent["com.apple.security.app-sandbox"]),
              !yes(ent["com.apple.security.get-task-allow"]),
              let permitted = ent["com.apple.security.application-groups"] as? [String], permitted.contains(group)
        else { throw workError(EPERM) }
    }
}
