// release/keychain-probe: one Keychain item, in the keychain named on the command line and no other, made, read or removed the way the
// app's Find key is (crates/recto-platform/src/find_key_mac.rs): a generic password, interaction refused. Built three times by
// tools/release/keychain-probe.sh, signed by the team twice and ad hoc once, to show what each may do with the item, and whether
// anything asks. It prints a status code and a time, never the item's bytes.
//
//   keychain-probe <add|read|delete> <keychain path> <global|per-call>
//
// `global` refuses interaction for the whole process (SecKeychainSetUserInteractionAllowed) and on each query; `per-call` only on
// each query (kSecUseAuthenticationUIFail), to see whether that alone is enough.
import Foundation
import Security

let build = "PROBE_BUILD"
let args = CommandLine.arguments
guard args.count == 4, ["add", "read", "delete"].contains(args[1]), ["global", "per-call"].contains(args[3]) else {
    FileHandle.standardError.write("usage: keychain-probe <add|read|delete> <keychain path> <global|per-call>\n".data(using: .utf8)!)
    exit(2)
}
let (mode, path, interaction) = (args[1], args[2], args[3])
if interaction == "global" {
    _ = SecKeychainSetUserInteractionAllowed(false)
}
var keychain: SecKeychain?
guard SecKeychainOpen(path, &keychain) == errSecSuccess, let keychain else {
    print("\(build) \(mode): the keychain couldn't be opened")
    exit(3)
}
var query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "io.github.kkollsga.RectoPDF.keychain-probe",
    kSecAttrAccount as String: "probe",
    kSecUseAuthenticationUI as String: kSecUseAuthenticationUIFail,
]
let started = Date()
var status: OSStatus
switch mode {
case "add":
    query[kSecUseKeychain as String] = keychain
    var bytes = [UInt8](repeating: 0, count: 32)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    query[kSecValueData as String] = Data(bytes)
    status = SecItemAdd(query as CFDictionary, nil)
case "read":
    query[kSecMatchSearchList as String] = [keychain]
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecSuccess, (found as? Data)?.count != 32 {
        print("\(build) read: the item isn't 32 bytes")
        exit(4)
    }
default:
    query[kSecMatchSearchList as String] = [keychain]
    status = SecItemDelete(query as CFDictionary)
}
let ms = Int(Date().timeIntervalSince(started) * 1000)
print("\(build) \(mode) (\(interaction)): status \(status) in \(ms) ms")
exit(status == errSecSuccess ? 0 : 1)
