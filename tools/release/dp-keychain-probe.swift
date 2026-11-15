// release/dp-keychain-probe: one item in the data-protection keychain, made, read, changed or removed the way the app's Find key is to be
// (the owner's decision, 2026-11-15: the key goes there, in the team's access group): a generic password with
// kSecUseDataProtectionKeychain, under a service of the probe's own so a run never meets the app's real item, in the access group named
// on the command line. tools/release/dp-keychain-probe.sh builds it several times with different signatures and entitlements and runs
// each against the same item. Nothing here turns prompts off: whether anything waits is what's measured.
//
//   dp-keychain-probe <add|read|update|delete> <access group>
//
// Prints `status=<OSStatus> ms=<milliseconds>`, and for a read that worked `sha=<first 8 hex of the bytes' SHA-256>`, so the script can
// tell that a later build reads what an earlier one made, and still does after other programs' tries. Never the item's bytes, which are
// the probe's own random ones anyway.
import CryptoKit
import Foundation
import Security

let build = "PROBE_BUILD"
let args = CommandLine.arguments
guard args.count == 3, ["add", "read", "update", "delete"].contains(args[1]) else {
    FileHandle.standardError.write("usage: dp-keychain-probe <add|read|update|delete> <access group>\n".data(using: .utf8)!)
    exit(2)
}
let (mode, group) = (args[1], args[2])
var query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "io.github.kkollsga.RectoPDF.dp-keychain-probe",
    kSecAttrAccount as String: "probe",
    kSecUseDataProtectionKeychain as String: true,
    kSecAttrAccessGroup as String: group,
]
func fresh() -> Data {
    var bytes = [UInt8](repeating: 0, count: 32)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    return Data(bytes)
}
let started = Date()
var status: OSStatus
var said = ""
switch mode {
case "add":
    query[kSecValueData as String] = fresh()
    query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    status = SecItemAdd(query as CFDictionary, nil)
case "read":
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecSuccess {
        guard let data = found as? Data, data.count == 32 else {
            print("\(build) read: the item isn't 32 bytes")
            exit(4)
        }
        said = " sha=" + SHA256.hash(data: data).prefix(4).map { String(format: "%02x", $0) }.joined()
    }
case "update":
    status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: fresh()] as CFDictionary)
default:
    status = SecItemDelete(query as CFDictionary)
}
let ms = Int(Date().timeIntervalSince(started) * 1000)
print("status=\(status) ms=\(ms)\(said)")
