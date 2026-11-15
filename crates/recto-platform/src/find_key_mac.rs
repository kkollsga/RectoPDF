//! **The Find key in the person's Keychain**, for the app signed by the team, and nobody else. The file-based login keychain, by `SecItem`: an item made by the app trusts the app's designated requirement and is in the team's partition, so the same team's next version reads it with no prompt (read: Apple's TN3137 and DTS's guidance, not measured with the team's signature here). A copy with any other signature can see the item is there and can't read or delete it: measured here on 2026-11-15 with two ad hoc builds of one identifier, a read was refused with `errSecAuthFailed` in 9 ms and a delete with `errSecInvalidOwnerEdit` in 5 ms, with nothing shown.
//!
//! **Nothing ever prompts:** interaction is refused for this process before the first call, so a locked Keychain or an item this copy may not read is a refusal, never a sheet. The caller then runs on a key in memory ([`super::FindKey::in_memory`]) and never adds a second item.
//!
//! Security.framework is opened only when this is asked, so no program that never asks loads it: the indexer, `recto` and the workers never do. Never called on the UI thread: the session asks on the indexer's thread, the first time a corpus is used.

use super::{FindKey, KEY_BYTES, Source};
use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

type CfRef = *const c_void;
type OsStatus = i32;

const ERR_SEC_SUCCESS: OsStatus = 0;
const ERR_SEC_ITEM_NOT_FOUND: OsStatus = -25300;
const ERR_SEC_DUPLICATE_ITEM: OsStatus = -25299;
const ERR_SEC_INTERACTION_NOT_ALLOWED: OsStatus = -25308;
const K_SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// The item's names. Not secret, and not about any document: any program can see the item is there.
pub const SERVICE: &str = "io.github.kkollsga.RectoPDF.find-text";
pub const ACCOUNT: &str = "find-text-key-1";

/// Why the Keychain gave no key. Each means the run goes on with a key in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// This copy isn't signed by the team, so it never asks: a development build, or one signed by someone else.
    NotTheTeam,
    /// Security.framework, or something in it, couldn't be had.
    Unavailable,
    /// The Keychain is locked, and unlocking it would need the person.
    Locked,
    /// There's an item this copy may not read: another signature made it. It isn't replaced, so items can't pile up.
    NotOurs(OsStatus),
    /// The item holds something that isn't a key.
    NotAKey,
    /// The OS gave no randomness for a new key.
    NoRandomness,
    Other(OsStatus),
}

impl Refused {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotTheTeam => "not_signed_by_the_team",
            Self::Unavailable => "keychain_unavailable",
            Self::Locked => "keychain_locked",
            Self::NotOurs(_) => "keychain_item_not_ours",
            Self::NotAKey => "keychain_item_not_a_key",
            Self::NoRandomness => "no_randomness",
            Self::Other(_) => "keychain_refused",
        }
    }
}

// The same shapes `surface_mac` declares these with, so the two declarations agree.
#[repr(C)]
struct CfDictionaryKeyCallBacks([*const c_void; 6]);
#[repr(C)]
struct CfDictionaryValueCallBacks([*const c_void; 5]);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(allocator: CfRef, bytes: *const u8, count: isize, encoding: u32, external: bool) -> CfRef;
    fn CFDataCreate(allocator: CfRef, bytes: *const u8, length: isize) -> CfRef;
    fn CFDataGetBytePtr(data: CfRef) -> *const u8;
    fn CFDataGetLength(data: CfRef) -> isize;
    fn CFGetTypeID(value: CfRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetCString(string: CfRef, buffer: *mut u8, size: isize, encoding: u32) -> bool;
    fn CFDictionaryCreate(
        allocator: CfRef,
        keys: *const CfRef,
        values: *const CfRef,
        count: isize,
        key_callbacks: *const CfDictionaryKeyCallBacks,
        value_callbacks: *const CfDictionaryValueCallBacks,
    ) -> CfRef;
    fn CFDictionaryGetValue(dict: CfRef, key: CfRef) -> CfRef;
    fn CFRelease(value: CfRef);
    static kCFTypeDictionaryKeyCallBacks: CfDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CfDictionaryValueCallBacks;
    static kCFBooleanTrue: CfRef;
}

/// What's used of Security.framework, found once.
struct Security {
    copy_matching: unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus,
    add: unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus,
    delete: unsafe extern "C" fn(CfRef) -> OsStatus,
    copy_self: unsafe extern "C" fn(u32, *mut CfRef) -> OsStatus,
    copy_signing_information: unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus,
    keys: Keys,
}

/// The dictionary keys and values a query names.
struct Keys {
    class: CfRef,
    generic_password: CfRef,
    service: CfRef,
    account: CfRef,
    return_data: CfRef,
    value_data: CfRef,
    match_limit: CfRef,
    match_limit_one: CfRef,
    authentication_ui: CfRef,
    authentication_ui_fail: CfRef,
    team_identifier: CfRef,
}

// SAFETY: the constants are immutable CFStrings of a framework that's never unloaded, and the functions are thread-safe.
unsafe impl Send for Security {}
// SAFETY: as above.
unsafe impl Sync for Security {}

fn security() -> Option<&'static Security> {
    static LOADED: OnceLock<Option<Security>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            const PATH: &CStr = c"/System/Library/Frameworks/Security.framework/Security";
            // SAFETY: a system framework by its full path, kept open for the life of the process.
            let handle = unsafe { libc::dlopen(PATH.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if handle.is_null() {
                return None;
            }
            let find = |name: &CStr| {
                // SAFETY: a symbol of the handle opened above.
                let found = unsafe { libc::dlsym(handle, name.as_ptr()) };
                (!found.is_null()).then_some(found)
            };
            // SAFETY: each constant's symbol is the address of a CFStringRef, read once.
            let key = |name: &CStr| find(name).map(|found| unsafe { *found.cast::<CfRef>() });
            // No prompt, ever, for anything this process asks of the Keychain: a refusal comes back instead. Deprecated, and still the one switch the file-based keychain heeds; every query also says `kSecUseAuthenticationUIFail`.
            let no_interaction = find(c"SecKeychainSetUserInteractionAllowed")?;
            // SAFETY: the function's real signature, `OSStatus (Boolean)`.
            let no_interaction: unsafe extern "C" fn(u8) -> OsStatus = unsafe { std::mem::transmute(no_interaction) };
            // SAFETY: as declared above.
            if unsafe { no_interaction(0) } != ERR_SEC_SUCCESS {
                return None;
            }
            // SAFETY: each is the named function's real signature.
            unsafe {
                Some(Security {
                    copy_matching: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus>(find(c"SecItemCopyMatching")?),
                    add: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus>(find(c"SecItemAdd")?),
                    delete: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef) -> OsStatus>(find(c"SecItemDelete")?),
                    copy_self: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, *mut CfRef) -> OsStatus>(find(c"SecCodeCopySelf")?),
                    copy_signing_information: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus>(find(
                        c"SecCodeCopySigningInformation",
                    )?),
                    keys: Keys {
                        class: key(c"kSecClass")?,
                        generic_password: key(c"kSecClassGenericPassword")?,
                        service: key(c"kSecAttrService")?,
                        account: key(c"kSecAttrAccount")?,
                        return_data: key(c"kSecReturnData")?,
                        value_data: key(c"kSecValueData")?,
                        match_limit: key(c"kSecMatchLimit")?,
                        match_limit_one: key(c"kSecMatchLimitOne")?,
                        authentication_ui: key(c"kSecUseAuthenticationUI")?,
                        authentication_ui_fail: key(c"kSecUseAuthenticationUIFail")?,
                        team_identifier: key(c"kSecCodeInfoTeamIdentifier")?,
                    },
                })
            }
        })
        .as_ref()
}

/// A CF object this owns, released when it goes.
struct Owned(CfRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an object this holds the one reference to.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn cf_string(text: &str) -> Owned {
    // SAFETY: the bytes live for the call, and CF copies them.
    Owned(unsafe { CFStringCreateWithBytes(std::ptr::null(), text.as_ptr(), text.len() as isize, K_CF_STRING_ENCODING_UTF8, false) })
}

fn dictionary(pairs: &[(CfRef, CfRef)]) -> Owned {
    let keys: Vec<CfRef> = pairs.iter().map(|pair| pair.0).collect();
    let values: Vec<CfRef> = pairs.iter().map(|pair| pair.1).collect();
    // SAFETY: two arrays of `count` CF objects, retained by the dictionary through the standard callbacks.
    Owned(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })
}

/// Whether this process is signed by `team`, by its own signature as the system checked it. A development build is ad hoc and has no team.
pub fn signed_by_team(team: &str) -> bool {
    let Some(sec) = security() else { return false };
    let mut code: CfRef = std::ptr::null();
    // SAFETY: an out-pointer for the call.
    if unsafe { (sec.copy_self)(0, &mut code) } != ERR_SEC_SUCCESS || code.is_null() {
        return false;
    }
    let code = Owned(code);
    let mut info: CfRef = std::ptr::null();
    // SAFETY: the code object above, and an out-pointer.
    if unsafe { (sec.copy_signing_information)(code.0, K_SEC_CS_SIGNING_INFORMATION, &mut info) } != ERR_SEC_SUCCESS || info.is_null() {
        return false;
    }
    let info = Owned(info);
    // SAFETY: a dictionary the call made; the value is borrowed from it.
    let found = unsafe { CFDictionaryGetValue(info.0, sec.keys.team_identifier) };
    // SAFETY: a CF object, checked for its type before it's read as a string.
    if found.is_null() || unsafe { CFGetTypeID(found) != CFStringGetTypeID() } {
        return false;
    }
    let mut buffer = [0u8; 64];
    // SAFETY: a buffer of its stated size.
    let read = unsafe { CFStringGetCString(found, buffer.as_mut_ptr(), buffer.len() as isize, K_CF_STRING_ENCODING_UTF8) };
    read && CStr::from_bytes_until_nul(&buffer).is_ok_and(|name| name.to_bytes() == team.as_bytes())
}

/// The key under `service` and `account`, made and added the first time. Only the app's own session calls this, once per run, off the UI thread, and only once [`signed_by_team`] has said yes: `load_for_the_app` is the one caller.
///
/// # Errors
///
/// [`Refused`]: the run goes on with a key in memory.
pub fn load_or_create(service: &str, account: &str) -> Result<FindKey, Refused> {
    let sec = security().ok_or(Refused::Unavailable)?;
    let (service, account) = (cf_string(service), cf_string(account));
    let query = dictionary(&[
        (sec.keys.class, sec.keys.generic_password),
        (sec.keys.service, service.0),
        (sec.keys.account, account.0),
        // SAFETY: a framework constant, never released.
        (sec.keys.return_data, unsafe { kCFBooleanTrue }),
        (sec.keys.match_limit, sec.keys.match_limit_one),
        (sec.keys.authentication_ui, sec.keys.authentication_ui_fail),
    ]);
    let mut found: CfRef = std::ptr::null();
    // SAFETY: a query dictionary and an out-pointer.
    let status = unsafe { (sec.copy_matching)(query.0, &mut found) };
    match status {
        ERR_SEC_SUCCESS => return key_from_data(Owned(found)),
        ERR_SEC_ITEM_NOT_FOUND => {}
        ERR_SEC_INTERACTION_NOT_ALLOWED => return Err(Refused::Locked),
        // `errSecAuthFailed` and the like: the item is there and isn't this copy's to read.
        other => return Err(Refused::NotOurs(other)),
    }
    let key = FindKey::fresh(Source::Keychain).map_err(|_| Refused::NoRandomness)?;
    // SAFETY: the key's bytes live for the call, and CF copies them. The copy is CF's to free, and isn't overwritten: CFData can't be told to.
    let data = Owned(unsafe { CFDataCreate(std::ptr::null(), key.bytes().as_ptr(), KEY_BYTES as isize) });
    let item = dictionary(&[
        (sec.keys.class, sec.keys.generic_password),
        (sec.keys.service, service.0),
        (sec.keys.account, account.0),
        (sec.keys.value_data, data.0),
        (sec.keys.authentication_ui, sec.keys.authentication_ui_fail),
    ]);
    // SAFETY: an item dictionary, and no result wanted.
    match unsafe { (sec.add)(item.0, std::ptr::null_mut()) } {
        ERR_SEC_SUCCESS => Ok(key),
        // Another run of the app added one first: that one is the key.
        ERR_SEC_DUPLICATE_ITEM => load_existing(sec, query.0),
        ERR_SEC_INTERACTION_NOT_ALLOWED => Err(Refused::Locked),
        other => Err(Refused::Other(other)),
    }
}

fn load_existing(sec: &Security, query: CfRef) -> Result<FindKey, Refused> {
    let mut found: CfRef = std::ptr::null();
    // SAFETY: a query dictionary and an out-pointer.
    match unsafe { (sec.copy_matching)(query, &mut found) } {
        ERR_SEC_SUCCESS => key_from_data(Owned(found)),
        status => Err(Refused::NotOurs(status)),
    }
}

fn key_from_data(data: Owned) -> Result<FindKey, Refused> {
    // SAFETY: a CF object, checked for its type before it's read as data.
    if data.0.is_null() || unsafe { CFGetTypeID(data.0) != CFDataGetTypeID() } {
        return Err(Refused::NotAKey);
    }
    // SAFETY: CFData's bytes, valid while `data` is held, of the length it says.
    let bytes = unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data.0), CFDataGetLength(data.0).max(0) as usize) };
    FindKey::from_slice(bytes, Source::Keychain).ok_or(Refused::NotAKey)
}

/// Removes an item. Only the prototype's own test of the real Keychain uses this, on its test item.
pub fn delete(service: &str, account: &str) -> Result<(), Refused> {
    let sec = security().ok_or(Refused::Unavailable)?;
    let (service, account) = (cf_string(service), cf_string(account));
    let query = dictionary(&[
        (sec.keys.class, sec.keys.generic_password),
        (sec.keys.service, service.0),
        (sec.keys.account, account.0),
    ]);
    // SAFETY: a query dictionary.
    match unsafe { (sec.delete)(query.0) } {
        ERR_SEC_SUCCESS | ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        status => Err(Refused::Other(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a test build counts as the team's, so a quiet run or a test would reach the person's Keychain.
    #[test]
    fn a_test_build_is_not_the_teams() {
        assert!(!signed_by_team("6GND8Z2C7Z"), "an ad hoc test binary claimed the team's signature");
    }

    /// The real Keychain, by hand only (`--ignored`), on one test item under a test service, removed at the end: the key made, read back the same, and gone. Never in a check run: those must not touch the person's Keychain.
    #[test]
    #[ignore = "touches the real login keychain; run by hand only"]
    fn the_keychain_keeps_a_key_and_lets_it_go() {
        const TEST_SERVICE: &str = "io.github.kkollsga.RectoPDF.TEST-probe-delete-me";
        const TEST_ACCOUNT: &str = "find-key-probe-test";
        let made = load_or_create(TEST_SERVICE, TEST_ACCOUNT).unwrap();
        let again = load_or_create(TEST_SERVICE, TEST_ACCOUNT).unwrap();
        assert_eq!(made.bytes(), again.bytes());
        delete(TEST_SERVICE, TEST_ACCOUNT).unwrap();
        let fresh = load_or_create(TEST_SERVICE, TEST_ACCOUNT).unwrap();
        assert_ne!(fresh.bytes(), made.bytes(), "a deleted item's key came back");
        delete(TEST_SERVICE, TEST_ACCOUNT).unwrap();
    }
}
