// release/keychain-probe: one Keychain item, made, read or removed the way the app's Find key is (crates/recto-platform/src/find_key_mac.rs):
// a generic password, under a service of the probe's own so a run never meets the app's real item, asked for through the default search
// list, as the app asks. tools/release/keychain-probe.sh builds it several times with different signatures and makes a throwaway
// keychain the default and the only one searched while it runs, so this touches nothing else. It prints the status code and the time,
// never the item's bytes.
//
//   keychain-probe <add|read|delete|add-open> <none|per-call|global> [<keychain>]
//   keychain-probe inspect <none|per-call|global> <keychain> <team>
//   keychain-probe <add-trusting|add-forged|add-then-forge> <none|per-call|global> <keychain> <team> <program>...
//
// With a keychain named, the lookup searches that keychain alone (kSecMatchSearchList) and an add puts the item there (kSecUseKeychain),
// as the app does with the login keychain; with none, the default search list and the default keychain. `add-open` adds with an
// access list that lets any application read the item, as a planter could.
//
// `inspect` reads the item as the app does, then prints what its access holds, names and counts only, never the item's bytes: each
// entry's authorizations, its programs (any, or each one's file name), and the partition list, then what the item-is-ours rule
// (crates/recto-platform/src/find_key_mac.rs) says of it, accept or refuse, with why. `add-trusting` adds with a list of trusted
// programs, given by path (`self` is this program); `add-forged` adds with that list and a partition entry of its own saying
// `teamid:<team>`; `add-then-forge` adds plainly, then tries to set an access with that partition entry on the item after.
//
// `global` refuses interaction for the whole process (SecKeychainSetUserInteractionAllowed), as the app does; `per-call` sets only
// kSecUseAuthenticationUIFail on the query, which Apple's header says governs data-protection items only; `none` refuses nothing, to show
// that a case would prompt.
import Foundation
import Security

let build = "PROBE_BUILD"
/// What a planting tried on the way, said on the status line.
var notes = [String]()
let args = CommandLine.arguments
let plain = ["add", "read", "delete", "add-open"].contains(args.count > 1 ? args[1] : "") && (args.count == 3 || args.count == 4)
let inspecting = args.count == 5 && args.count > 1 && args[1] == "inspect"
let planting = args.count >= 6 && ["add-trusting", "add-forged", "add-then-forge"].contains(args[1])
guard plain || inspecting || planting, ["none", "per-call", "global"].contains(args[2]) else {
    FileHandle.standardError.write("usage: see tools/release/keychain-probe.swift\n".data(using: .utf8)!)
    exit(2)
}
let (mode, interaction) = (args[1], args[2])
if interaction == "global" {
    _ = SecKeychainSetUserInteractionAllowed(false)
}
var query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "io.github.kkollsga.RectoPDF.keychain-probe",
    kSecAttrAccount as String: "probe",
]
if interaction == "per-call" {
    query[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
}
var named: SecKeychain?
if args.count >= 4 {
    // A keychain that isn't there opens anyway, and an add to it goes to the default keychain instead: seen here, 2026-11-15.
    guard FileManager.default.fileExists(atPath: args[3]) else {
        print("\(build) \(mode): no keychain at that path")
        exit(3)
    }
    guard SecKeychainOpen(args[3], &named) == errSecSuccess, named != nil else {
        print("\(build) \(mode): the keychain couldn't be opened")
        exit(3)
    }
}
/// The file name of a trusted program's path, as SecTrustedApplicationCopyData gives it (a C string).
func programName(_ app: SecTrustedApplication) -> String {
    var data: CFData?
    guard SecTrustedApplicationCopyData(app, &data) == errSecSuccess, let data = data as Data? else { return "?" }
    let path = String(decoding: data.prefix { $0 != 0 }, as: UTF8.self)
    return (path as NSString).lastPathComponent
}

/// A partition entry's description: a plist of {Partitions: [...]}, hex-encoded, as the file keychain keeps it.
func partitions(_ description: String) -> [String]? {
    let read = { (data: Data) -> [String]? in
        let plist = try? PropertyListSerialization.propertyList(from: data, options: [], format: nil)
        return (plist as? [String: Any])?["Partitions"] as? [String]
    }
    var bytes = [UInt8]()
    var chars = Array(description.utf8)
    while chars.count >= 2, let byte = UInt8(String(decoding: chars.prefix(2), as: UTF8.self), radix: 16) {
        bytes.append(byte)
        chars.removeFirst(2)
    }
    // Hex, as `security dump-keychain -a` shows it; failing that, the plist as it is.
    return (chars.isEmpty ? read(Data(bytes)) : nil) ?? read(Data(description.utf8))
}

func hexPartitions(_ list: [String]) -> String {
    let data = try! PropertyListSerialization.data(fromPropertyList: ["Partitions": list], format: .xml, options: 0)
    return data.map { String(format: "%02x", $0) }.joined()
}

/// What's read of an item's access, and the rule's word on it.
func describe(_ access: SecAccess, team: String) -> String {
    var list: CFArray?
    guard SecAccessCopyACLList(access, &list) == errSecSuccess, let acls = list as? [SecACL] else { return "acls=unreadable rule=refuse(unreadable)" }
    var parts = ["acls=\(acls.count)"]
    var found: [String]? = nil
    var partitionEntries = 0
    var open = false
    var noPrograms = false
    var readers = 0
    for acl in acls {
        let auths = (SecACLCopyAuthorizations(acl) as? [String]) ?? []
        var apps: CFArray?
        var description: CFString?
        var prompt = SecKeychainPromptSelector()
        let read = SecACLCopyContents(acl, &apps, &description, &prompt)
        let short = auths.map { $0.replacingOccurrences(of: "ACLAuthorization", with: "") }.joined(separator: "+")
        if auths.contains(kSecACLAuthorizationPartitionID as String) {
            partitionEntries += 1
            found = partitions((description as String?) ?? "")
            parts.append("[\(short): \(found.map { $0.joined(separator: ",") } ?? "unreadable")]")
            continue
        }
        let programs: String
        if read != errSecSuccess {
            programs = "unreadable(\(read))"
        } else if let apps = apps as? [SecTrustedApplication] {
            programs = apps.isEmpty ? "none" : "\(apps.count):" + apps.map(programName).joined(separator: ",")
        } else {
            programs = "any"
        }
        let decrypts = auths.contains(kSecACLAuthorizationDecrypt as String) || auths.contains(kSecACLAuthorizationAny as String)
        if decrypts {
            readers += 1
            if apps == nil { open = true }
            if let list = apps as? [SecTrustedApplication], list.isEmpty { noPrograms = true }
        }
        parts.append("[\(short): \(programs)]")
    }
    let rule: String
    if open {
        rule = "refuse(open to any application)"
    } else if readers == 0 || noPrograms {
        rule = "refuse(no program named)"
    } else if partitionEntries != 1 || found != ["teamid:\(team)"] {
        rule = "refuse(partition \(found.map { $0.joined(separator: ",") } ?? "unreadable"))"
    } else {
        rule = "accept"
    }
    parts.append("rule=\(rule)")
    return parts.joined(separator: " ")
}

/// An access naming the given programs (`self` is this one), and, if asked, a partition entry of its own.
func access(trusting programs: [String], forging team: String?) -> SecAccess? {
    var trusted = [SecTrustedApplication]()
    for program in programs {
        var app: SecTrustedApplication?
        let made = program == "self" ? SecTrustedApplicationCreateFromPath(nil, &app) : SecTrustedApplicationCreateFromPath(program, &app)
        guard made == errSecSuccess, let app else { print("\(build): no trusted program for \(program)"); return nil }
        trusted.append(app)
    }
    var access: SecAccess?
    guard SecAccessCreate("keychain-probe" as CFString, trusted as CFArray, &access) == errSecSuccess, let access else { return nil }
    if let team {
        var acl: SecACL?
        let made = SecACLCreateWithSimpleContents(access, nil, hexPartitions(["teamid:\(team)"]) as CFString, SecKeychainPromptSelector(), &acl)
        let set = acl.map { SecACLUpdateAuthorizations($0, [kSecACLAuthorizationPartitionID] as CFArray) } ?? made
        notes.append("forge-entry=\(made) forge-authorization=\(set)")
    }
    return access
}

let started = Date()
var status: OSStatus
switch mode {
case "add", "add-open":
    var bytes = [UInt8](repeating: 0, count: 32)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    query[kSecValueData as String] = Data(bytes)
    if let named { query[kSecUseKeychain as String] = named }
    if mode == "add-open" {
        // Any application may read it: the decrypt entries' application list set to none, which means any.
        var access: SecAccess?
        guard SecAccessCreate("keychain-probe" as CFString, nil, &access) == errSecSuccess, let access else {
            print("\(build) add-open: no access list"); exit(3)
        }
        let acls = SecAccessCopyMatchingACLList(access, kSecACLAuthorizationDecrypt) as? [SecACL] ?? []
        for acl in acls {
            var apps: CFArray?
            var description: CFString?
            var prompt = SecKeychainPromptSelector()
            _ = SecACLCopyContents(acl, &apps, &description, &prompt)
            _ = SecACLSetContents(acl, nil, description ?? ("keychain-probe" as CFString), prompt)
        }
        query[kSecAttrAccess as String] = access
    }
    status = SecItemAdd(query as CFDictionary, nil)
case "read":
    if let named { query[kSecMatchSearchList as String] = [named] }
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecSuccess, (found as? Data)?.count != 32 {
        print("\(build) read: the item isn't 32 bytes")
        exit(4)
    }
case "inspect":
    query[kSecMatchSearchList as String] = [named!]
    query[kSecReturnData as String] = true
    query[kSecReturnRef as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    status = SecItemCopyMatching(query as CFDictionary, &found)
    let ms = Int(Date().timeIntervalSince(started) * 1000)
    guard status == errSecSuccess, let found = found as? [String: Any] else {
        print("status=\(status) ms=\(ms)")
        exit(0)
    }
    guard (found[kSecValueData as String] as? Data)?.count == 32, let item = found[kSecValueRef as String] else {
        print("\(build) inspect: the item isn't 32 bytes, or came with no reference")
        exit(4)
    }
    var itemAccess: SecAccess?
    let copied = SecKeychainItemCopyAccess(item as! SecKeychainItem, &itemAccess)
    print("status=\(status) ms=\(ms) " + (itemAccess.map { describe($0, team: args[4]) } ?? "access=unreadable(\(copied)) rule=refuse(unreadable)"))
    exit(0)
case "add-trusting", "add-forged", "add-then-forge":
    var bytes = [UInt8](repeating: 0, count: 32)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    query[kSecValueData as String] = Data(bytes)
    query[kSecUseKeychain as String] = named!
    let programs = Array(args[5...])
    if mode != "add-then-forge" {
        guard let made = access(trusting: programs, forging: mode == "add-forged" ? args[4] : nil) else { exit(3) }
        query[kSecAttrAccess as String] = made
        status = SecItemAdd(query as CFDictionary, nil)
    } else {
        query[kSecReturnRef as String] = true
        var item: CFTypeRef?
        status = SecItemAdd(query as CFDictionary, &item)
        if status == errSecSuccess, let item, let forged = access(trusting: programs, forging: args[4]) {
            notes.append("forge-set=\(SecKeychainItemSetAccess(item as! SecKeychainItem, forged))")
        }
    }
default:
    if let named { query[kSecMatchSearchList as String] = [named] }
    status = SecItemDelete(query as CFDictionary)
}
let ms = Int(Date().timeIntervalSince(started) * 1000)
print((["status=\(status) ms=\(ms)"] + notes).joined(separator: " "))
