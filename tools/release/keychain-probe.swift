// release/keychain-probe: one Keychain item, made, read or removed the way the app's Find key is (crates/recto-platform/src/find_key_mac.rs):
// a generic password, under a service of the probe's own so a run never meets the app's real item, asked for through the default search
// list, as the app asks. tools/release/keychain-probe.sh builds it several times with different signatures and makes a throwaway
// keychain the default and the only one searched while it runs, so this touches nothing else. It prints the status code and the time,
// never the item's bytes.
//
//   keychain-probe <add|read|delete> <none|per-call|global>
//
// `global` refuses interaction for the whole process (SecKeychainSetUserInteractionAllowed), as the app does; `per-call` sets only
// kSecUseAuthenticationUIFail on the query, which Apple's header says governs data-protection items only; `none` refuses nothing, to show
// that a case would prompt.
import Foundation
import Security

let build = "PROBE_BUILD"
let args = CommandLine.arguments
guard args.count == 3, ["add", "read", "delete"].contains(args[1]), ["none", "per-call", "global"].contains(args[2]) else {
    FileHandle.standardError.write("usage: keychain-probe <add|read|delete> <none|per-call|global>\n".data(using: .utf8)!)
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
let started = Date()
var status: OSStatus
switch mode {
case "add":
    var bytes = [UInt8](repeating: 0, count: 32)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    query[kSecValueData as String] = Data(bytes)
    status = SecItemAdd(query as CFDictionary, nil)
case "read":
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecSuccess, (found as? Data)?.count != 32 {
        print("\(build) read: the item isn't 32 bytes")
        exit(4)
    }
default:
    status = SecItemDelete(query as CFDictionary)
}
let ms = Int(Date().timeIntervalSince(started) * 1000)
print("status=\(status) ms=\(ms)")
