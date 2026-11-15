//! **The Find key in the person's data-protection keychain, in the app's own access group** (the owner's decision, 2026-11-15, after the login keychain was shown not to hold the claim: runs 43706807396 and 43708234469, a program of this user's could plant an item the app read, or overwrite the app's own; every run's numbers are in `docs/engineering/search-index.md`). `SecItem` with `kSecUseDataProtectionKeychain`, a generic password in the group `<team>.<bundle id>` (the app ID, from `recto_update`'s `TEAM_ID` and `BUNDLE_ID`, passed in as [`super::App`]), readable after the first unlock and on this Mac only. An item in an access group can be made or read only by a program whose signature claims that group, and the claim is a restricted entitlement a provisioning profile must authorize (Apple's TN3137 and TN3125: "other developers can't impersonate your app in order to steal its keychain items"); the release's profile is the team's Developer ID profile for the app ID, embedded in the bundle, with the entitlements on the main executable only. A program without the entitlement, a development build or any other program of this user's, is refused with `errSecMissingEntitlement` (-34018) or finds nothing; the release preflight's probe (`tools/release/dp-keychain-probe.sh`) measures each of those on the team's signature, the profile and the runner, and that a later build reads what an earlier one made with no prompt: run 43754866746, every one held (`docs/engineering/search-index.md`).
//!
//! **Asked only by a copy that is the team's release, checked whole:** [`signed_as_a_release`] holds this process's code on disk to the Developer ID requirement the updater uses ([`crate::signing`]), validating the executable and every sealed resource, nested code included, so a copy with a resource changed, or an ad hoc signature claiming the team, never asks.
//!
//! **Nothing prompts.** The data-protection keychain asks nothing of the person for an item with no access control, which this is; the per-query `kSecUseAuthenticationUIFail` is set as well, and governs that keychain. The probe runs with neither of the app's guards, so what it measures isn't owed to them. The process-wide switch, `SecKeychainSetUserInteractionAllowed(false)`, is still set before the first call and never set back, as it was for the login keychain (measured then, run 43699949398): it silences the file-based keychain's prompts, of which the app should raise none now, so it's kept as a guard against one that would come of a mistake, at no cost; a test holds the crates to naming it nowhere else. A refusal of any kind leaves the run on a key in memory for now ([`super::FindKey::in_memory_for_now`]), and it's said only where `qa state` shows the key's plan: never to the person as an error, never to an agent.
//!
//! **CoreFoundation's copies:** adding the key copies it into a `CFData`, and reading it hands back another; both are CoreFoundation's to free, and neither is overwritten, as `CFData` can't be told to. Each lives for the one call. The key itself is copied straight from that data into [`FindKey`]'s own memory.
//!
//! Security.framework is opened only when this is asked, so no program that never asks loads it: the indexer, `recto` and the workers never do, and their binaries are held to having none of this code. Its one caller is [`super::load_for_the_app`]; what makes that off the UI thread is the indexer's own rule (`recto_control::indexing`, which takes the key on no main thread).

use super::{FindKey, KEY_BYTES, Source};
use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

type CfRef = *const c_void;
type OsStatus = i32;
/// CoreFoundation's `Boolean`, an unsigned char.
type CfBoolean = u8;

const ERR_SEC_SUCCESS: OsStatus = 0;
const ERR_SEC_ITEM_NOT_FOUND: OsStatus = -25300;
const ERR_SEC_DUPLICATE_ITEM: OsStatus = -25299;
#[cfg(test)]
const ERR_SEC_INTERACTION_NOT_ALLOWED: OsStatus = -25308;
/// `errSecMissingEntitlement`: this program's signature doesn't claim the access group, so it may neither read nor make an item in it.
const ERR_SEC_MISSING_ENTITLEMENT: OsStatus = -34018;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// `kSecCSCheckNestedCode | kSecCSStrictValidate`: on top of the default, which validates the executable and every sealed resource, the nested code (the renderer's helper, its library) and strict structure, as `codesign --verify --deep --strict` does.
const K_SEC_CS_RELEASE_CHECK: u32 = (1 << 3) | (1 << 4);

/// The item's names. Not secret, and not about any document: any program can see the item is there.
pub(super) const SERVICE: &str = "io.github.kkollsga.RectoPDF.find-text";
pub(super) const ACCOUNT: &str = "find-text-key-1";

/// Why the Keychain gave no key. Each means the run goes on with a key in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// This copy isn't the team's release, checked whole, so it never asks: a development build, one signed by someone else, or one changed since it was signed.
    NotTheTeam,
    /// Security.framework, or something in it, couldn't be had.
    Unavailable,
    /// There may be an item, and this copy can't read it now (`errSecInteractionNotAllowed` before the Mac's first unlock, or any status but "no item" and "not entitled"). Nothing is added over it.
    Unreadable(OsStatus),
    /// This copy's signature doesn't claim the app's access group (`errSecMissingEntitlement`): a release built without its profile and entitlements. Nothing is made: it's a key in memory for now, and the lasting key's index is left for a copy that has them.
    NotEntitled,
    /// The item holds something that isn't a key.
    NotAKey,
    /// The OS gave no randomness for a new key.
    NoRandomness,
}

impl Refused {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::NotTheTeam => "not_signed_by_the_team",
            Self::Unavailable => "keychain_unavailable",
            Self::Unreadable(_) => "keychain_item_unreadable",
            Self::NotEntitled => "keychain_not_entitled",
            Self::NotAKey => "keychain_item_not_a_key",
            Self::NoRandomness => "no_randomness",
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
    fn CFStringCreateWithBytes(allocator: CfRef, bytes: *const u8, count: isize, encoding: u32, external: CfBoolean) -> CfRef;
    fn CFDataCreate(allocator: CfRef, bytes: *const u8, length: isize) -> CfRef;
    fn CFDataGetBytePtr(data: CfRef) -> *const u8;
    fn CFDataGetLength(data: CfRef) -> isize;
    fn CFGetTypeID(value: CfRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDictionaryCreate(
        allocator: CfRef,
        keys: *const CfRef,
        values: *const CfRef,
        count: isize,
        key_callbacks: *const CfDictionaryKeyCallBacks,
        value_callbacks: *const CfDictionaryValueCallBacks,
    ) -> CfRef;
    #[cfg(test)]
    fn CFURLCreateFromFileSystemRepresentation(allocator: CfRef, path: *const u8, length: isize, directory: CfBoolean) -> CfRef;
    fn CFRelease(value: CfRef);
    #[cfg(test)]
    fn CFDictionaryGetValue(dictionary: CfRef, key: CfRef) -> CfRef;
    static kCFTypeDictionaryKeyCallBacks: CfDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CfDictionaryValueCallBacks;
    static kCFBooleanTrue: CfRef;
}

/// What's used of Security.framework, found once.
struct Security {
    copy_matching: unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus,
    add: unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus,
    copy_self: unsafe extern "C" fn(u32, *mut CfRef) -> OsStatus,
    copy_static: unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus,
    #[cfg(test)]
    static_with_path: unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus,
    requirement: unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus,
    check_static: unsafe extern "C" fn(CfRef, u32, CfRef) -> OsStatus,
    /// Whether the process-wide switch is off, as it's set before anything else is asked. Not having it doesn't stop the key being taken (the security read's F1): the data-protection keychain doesn't prompt for an item like this one, and it's said in `qa state` if so.
    switch_off: bool,
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
    data_protection: CfRef,
    access_group: CfRef,
    accessible: CfRef,
    after_first_unlock_this_device: CfRef,
    /// What would carry an item to the person's other devices through iCloud: named only for the test that the key never asks for it.
    #[cfg(test)]
    synchronizable: CfRef,
}

// SAFETY: the constants are immutable CFStrings of a framework that's never unloaded, and the functions are thread-safe.
unsafe impl Send for Security {}
// SAFETY: as above.
unsafe impl Sync for Security {}

/// The one place the process-wide switch is touched: off, before anything else is asked, and never on again.
const SWITCH: &CStr = c"SecKeychainSetUserInteractionAllowed";

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
            let key = |name: &CStr| find(name).map(|found| unsafe { *found.cast::<CfRef>() }).filter(|value| !value.is_null());
            let switch_off = find(SWITCH).is_some_and(|found| {
                // SAFETY: the function's real signature, `OSStatus (Boolean)`.
                let no_interaction: unsafe extern "C" fn(CfBoolean) -> OsStatus = unsafe { std::mem::transmute(found) };
                // SAFETY: as declared above. Refused for the whole process, for good: see the module's notes.
                unsafe { no_interaction(0) == ERR_SEC_SUCCESS }
            });
            // SAFETY: each is the named function's real signature.
            unsafe {
                Some(Security {
                    copy_matching: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus>(find(c"SecItemCopyMatching")?),
                    add: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, *mut CfRef) -> OsStatus>(find(c"SecItemAdd")?),
                    copy_self: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, *mut CfRef) -> OsStatus>(find(c"SecCodeCopySelf")?),
                    copy_static: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus>(find(c"SecCodeCopyStaticCode")?),
                    #[cfg(test)]
                    static_with_path: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus>(find(
                        c"SecStaticCodeCreateWithPath",
                    )?),
                    requirement: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, u32, *mut CfRef) -> OsStatus>(find(
                        c"SecRequirementCreateWithString",
                    )?),
                    check_static: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, u32, CfRef) -> OsStatus>(find(c"SecStaticCodeCheckValidity")?),
                    switch_off,
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
                        data_protection: key(c"kSecUseDataProtectionKeychain")?,
                        access_group: key(c"kSecAttrAccessGroup")?,
                        accessible: key(c"kSecAttrAccessible")?,
                        after_first_unlock_this_device: key(c"kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly")?,
                        #[cfg(test)]
                        synchronizable: key(c"kSecAttrSynchronizable")?,
                    },
                })
            }
        })
        .as_ref()
}

/// A CF object this owns, released when it goes. Null when CF made nothing.
struct Owned(CfRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an object this holds the one reference to.
            unsafe { CFRelease(self.0) };
        }
    }
}

impl Owned {
    /// Itself, unless CF made nothing.
    fn made(self) -> Option<Self> {
        (!self.0.is_null()).then_some(self)
    }
}

fn cf_string(text: &str) -> Option<Owned> {
    // SAFETY: the bytes live for the call, and CF copies them.
    Owned(unsafe { CFStringCreateWithBytes(std::ptr::null(), text.as_ptr(), text.len() as isize, K_CF_STRING_ENCODING_UTF8, 0) }).made()
}

fn dictionary(pairs: &[(CfRef, CfRef)]) -> Option<Owned> {
    if pairs.iter().any(|(key, value)| key.is_null() || value.is_null()) {
        return None;
    }
    let keys: Vec<CfRef> = pairs.iter().map(|pair| pair.0).collect();
    let values: Vec<CfRef> = pairs.iter().map(|pair| pair.1).collect();
    // SAFETY: two arrays of `count` CF objects, none null, retained by the dictionary through the standard callbacks.
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
    .made()
}

/// What checking static code against a requirement said, with the executable, every sealed resource and the nested code validated ([`K_SEC_CS_RELEASE_CHECK`]): the check's status, or `None` when it couldn't be made (the requirement didn't compile).
fn static_code_check(sec: &Security, code: &Owned, requirement: &str) -> Option<OsStatus> {
    let text = cf_string(requirement)?;
    let mut compiled: CfRef = std::ptr::null();
    // SAFETY: a string and an out-pointer.
    if unsafe { (sec.requirement)(text.0, 0, &mut compiled) } != ERR_SEC_SUCCESS {
        return None;
    }
    let compiled = Owned(compiled).made()?;
    // SAFETY: a static code object and a requirement, both held.
    Some(unsafe { (sec.check_static)(code.0, K_SEC_CS_RELEASE_CHECK, compiled.0) })
}

/// Whether this copy is the team's release, as one of three answers: only a check that was made and failed says it isn't (the security read's F1). One that couldn't be made says nothing either way, and the run takes a key in memory for now, which never builds over the lasting key's index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Release {
    Yes,
    No,
    Unknown,
}

/// What a check's status, or its not being made, answers.
fn release_answer(status: Option<OsStatus>) -> Release {
    match status {
        None => Release::Unknown,
        Some(ERR_SEC_SUCCESS) => Release::Yes,
        Some(_) => Release::No,
    }
}

/// Whether the process-wide switch is off: not, if Security.framework couldn't be had, or the switch wasn't there or refused.
pub(super) fn prompts_switched_off() -> bool {
    security().is_some_and(|sec| sec.switch_off)
}

/// **Whether this running copy is the team's release, checked whole:** its code on disk, the bundle it was launched from, validated against its signature (the executable, every sealed resource, nested code, strictly) and held to the Developer ID requirement the updater uses. A development build is ad hoc and fails; so does an ad hoc signature claiming the team, and a release with a resource changed. Costs a hash of the whole bundle, once per run, on the thread that takes the key.
pub(super) fn signed_as_a_release(team: &str) -> Release {
    let Some(sec) = security() else { return Release::Unknown };
    let mut running: CfRef = std::ptr::null();
    // SAFETY: an out-pointer for the call.
    if unsafe { (sec.copy_self)(0, &mut running) } != ERR_SEC_SUCCESS {
        return Release::Unknown;
    }
    let Some(running) = Owned(running).made() else { return Release::Unknown };
    let mut on_disk: CfRef = std::ptr::null();
    // SAFETY: the running code above, and an out-pointer.
    if unsafe { (sec.copy_static)(running.0, 0, &mut on_disk) } != ERR_SEC_SUCCESS {
        return Release::Unknown;
    }
    let Some(on_disk) = Owned(on_disk).made() else { return Release::Unknown };
    release_answer(static_code_check(sec, &on_disk, &crate::signing::developer_id_requirement(team)))
}

/// Whether the code at `path` meets `requirement`, checked as [`signed_as_a_release`] checks this copy: for its tests, on a bundle they make.
#[cfg(test)]
fn path_meets(path: &std::path::Path, requirement: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(sec) = security() else { return false };
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: the path's bytes live for the call, and CF copies them.
    let Some(url) = Owned(unsafe { CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 1) }).made() else {
        return false;
    };
    let mut code: CfRef = std::ptr::null();
    // SAFETY: a URL and an out-pointer.
    if unsafe { (sec.static_with_path)(url.0, 0, &mut code) } != ERR_SEC_SUCCESS {
        return false;
    }
    Owned(code)
        .made()
        .is_some_and(|code| release_answer(static_code_check(sec, &code, requirement)) == Release::Yes)
}

/// The key under `service` and `account` in the access `group`, made and added the first time: at most three Keychain calls, a look, an add, and a look again if another run added one first. Only [`super::load_for_the_app`] calls this, and only once [`signed_as_a_release`] has said yes.
pub(super) fn load_or_create(service: &str, account: &str, group: &str) -> Result<FindKey, Refused> {
    let sec = security().ok_or(Refused::Unavailable)?;
    let names = Names {
        service: cf_string(service).ok_or(Refused::Unavailable)?,
        account: cf_string(account).ok_or(Refused::Unavailable)?,
        group: cf_string(group).ok_or(Refused::Unavailable)?,
    };
    let query = lookup(sec, &names).ok_or(Refused::Unavailable)?;
    let mut found: CfRef = std::ptr::null();
    // SAFETY: a query dictionary and an out-pointer.
    let status = unsafe { (sec.copy_matching)(query.0, &mut found) };
    let found = Owned(found);
    match after_lookup(status) {
        Lookup::Found => return key_from_data(&found),
        Lookup::Absent => {}
        Lookup::Refused(status) => return Err(refusal(status)),
    }
    let key = FindKey::fresh(Source::Keychain).map_err(|_| Refused::NoRandomness)?;
    // SAFETY: the key's bytes live for the call, and CF copies them. The copy is CF's to free, and isn't overwritten: CFData can't be told to.
    let data = Owned(unsafe { CFDataCreate(std::ptr::null(), key.bytes().as_ptr(), KEY_BYTES as isize) })
        .made()
        .ok_or(Refused::Unavailable)?;
    let item = new_item(sec, &names, &data).ok_or(Refused::Unavailable)?;
    // SAFETY: an item dictionary, and no result wanted.
    match unsafe { (sec.add)(item.0, std::ptr::null_mut()) } {
        ERR_SEC_SUCCESS => Ok(key),
        // Another run of the app added one first: that one is the key.
        ERR_SEC_DUPLICATE_ITEM => load_existing(sec, query.0),
        status => Err(refusal(status)),
    }
}

/// The item's names, as CoreFoundation strings.
struct Names {
    service: Owned,
    account: Owned,
    group: Owned,
}

/// The look for the item: in the data-protection keychain, in the app's access group, its data wanted, one match, no prompt asked for.
fn lookup(sec: &Security, names: &Names) -> Option<Owned> {
    dictionary(&[
        (sec.keys.class, sec.keys.generic_password),
        (sec.keys.service, names.service.0),
        (sec.keys.account, names.account.0),
        // SAFETY: a framework constant, never released.
        (sec.keys.data_protection, unsafe { kCFBooleanTrue }),
        (sec.keys.access_group, names.group.0),
        // SAFETY: as above.
        (sec.keys.return_data, unsafe { kCFBooleanTrue }),
        (sec.keys.match_limit, sec.keys.match_limit_one),
        (sec.keys.authentication_ui, sec.keys.authentication_ui_fail),
    ])
}

/// The new item: in the data-protection keychain, in the app's access group, readable after the Mac's first unlock and on this Mac only.
fn new_item(sec: &Security, names: &Names, data: &Owned) -> Option<Owned> {
    dictionary(&[
        (sec.keys.class, sec.keys.generic_password),
        (sec.keys.service, names.service.0),
        (sec.keys.account, names.account.0),
        // SAFETY: a framework constant, never released.
        (sec.keys.data_protection, unsafe { kCFBooleanTrue }),
        (sec.keys.access_group, names.group.0),
        (sec.keys.accessible, sec.keys.after_first_unlock_this_device),
        (sec.keys.value_data, data.0),
        (sec.keys.authentication_ui, sec.keys.authentication_ui_fail),
    ])
}

/// What a status that isn't the answer wanted means: not entitled to the group, or not readable now.
fn refusal(status: OsStatus) -> Refused {
    match status {
        ERR_SEC_MISSING_ENTITLEMENT => Refused::NotEntitled,
        status => Refused::Unreadable(status),
    }
}

fn load_existing(sec: &Security, query: CfRef) -> Result<FindKey, Refused> {
    let mut found: CfRef = std::ptr::null();
    // SAFETY: a query dictionary and an out-pointer.
    let status = unsafe { (sec.copy_matching)(query, &mut found) };
    let found = Owned(found);
    match after_lookup(status) {
        Lookup::Found => key_from_data(&found),
        Lookup::Absent | Lookup::Refused(_) => Err(refusal(status)),
    }
}

/// What a lookup's status means for what comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lookup {
    Found,
    /// No item: the only answer after which one is added.
    Absent,
    /// Anything else: nothing is added.
    Refused(OsStatus),
}

fn after_lookup(status: OsStatus) -> Lookup {
    match status {
        ERR_SEC_SUCCESS => Lookup::Found,
        ERR_SEC_ITEM_NOT_FOUND => Lookup::Absent,
        other => Lookup::Refused(other),
    }
}

/// The key an item's data holds: exactly 32 bytes, or nothing. Empty data has no bytes at all (CoreFoundation's pointer is null then), so the length and the pointer are looked at before any slice is made.
fn key_from_data(data: &Owned) -> Result<FindKey, Refused> {
    // SAFETY: a CF object, checked for its type before it's read as data.
    if data.0.is_null() || unsafe { CFGetTypeID(data.0) != CFDataGetTypeID() } {
        return Err(Refused::NotAKey);
    }
    // SAFETY: a CFData, as checked above.
    let (length, bytes) = unsafe { (CFDataGetLength(data.0), CFDataGetBytePtr(data.0)) };
    if length != KEY_BYTES as isize || bytes.is_null() {
        return Err(Refused::NotAKey);
    }
    // SAFETY: CFData's bytes, non-null and of the length checked, valid while `data` is held.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, KEY_BYTES) };
    FindKey::from_slice(bytes, Source::Keychain).ok_or(Refused::NotAKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a test build counts as the team's release, so a quiet run or a test would reach the person's Keychain.
    #[test]
    fn a_test_build_is_not_a_release() {
        assert_eq!(
            signed_as_a_release("6GND8Z2C7Z"),
            Release::No,
            "an ad hoc test binary passed as the team's release, or its check wasn't made"
        );
    }

    /// Failure: an item whose data is empty or isn't 32 bytes is read as a key, or reading it makes a slice from CoreFoundation's null pointer (undefined behaviour: empty `CFData` has none). Fed straight to the read path, with no Keychain.
    #[test]
    fn data_that_isnt_a_key_is_refused_before_any_slice() {
        for length in [0, 1, 31, 33, 64] {
            let bytes = vec![7u8; length];
            // SAFETY: the bytes live for the call, and CF copies them.
            let data = Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), length as isize) });
            assert!(matches!(key_from_data(&data), Err(Refused::NotAKey)), "{length} bytes read as a key");
        }
        let bytes = [9u8; KEY_BYTES];
        // SAFETY: as above.
        let data = Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), KEY_BYTES as isize) });
        assert_eq!(key_from_data(&data).expect("32 bytes are a key").bytes(), &bytes);
        let string = cf_string("not data").unwrap();
        assert!(matches!(key_from_data(&string), Err(Refused::NotAKey)), "a string read as a key");
        assert!(matches!(key_from_data(&Owned(std::ptr::null())), Err(Refused::NotAKey)));
    }

    /// Failure: the check before the Keychain is asked passes code whose resources were changed after it was signed, or code that meets no requirement: the flags didn't validate what they say. On a bundle made here and signed ad hoc: it meets `always`, and fails it once a sealed resource is changed; it never meets the team's requirement.
    #[test]
    fn a_bundle_with_a_resource_changed_fails_the_check() {
        let temp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-release-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let bundle = temp.join("Probe.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>io.github.kkollsga.RectoPDF.release-check-probe</string><key>CFBundleExecutable</key><string>probe</string></dict></plist>"#,
        )
        .unwrap();
        std::fs::copy("/usr/bin/true", bundle.join("Contents/MacOS/probe")).unwrap();
        std::fs::write(bundle.join("Contents/Resources/words.txt"), b"as signed").unwrap();
        let signed = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&bundle)
            .output()
            .unwrap();
        assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
        assert!(path_meets(&bundle, "always"), "a bundle as signed failed the check");
        assert!(
            !path_meets(&bundle, &crate::signing::developer_id_requirement("6GND8Z2C7Z")),
            "an ad hoc bundle met the team's requirement"
        );
        std::fs::write(bundle.join("Contents/Resources/words.txt"), b"changed after").unwrap();
        assert!(!path_meets(&bundle, "always"), "a bundle with a resource changed passed the check");
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Failure (the security read's F1): a release check that couldn't be made (a requirement that didn't compile) reads as "not the team's release", so a run that may simply have failed to look builds over the person's lasting index; or one that was made and failed reads as unknown, so a development build waits for a Keychain it will never have. Only a made check's failure is "no".
    #[test]
    fn only_a_check_that_was_made_and_failed_says_not_the_release() {
        assert_eq!(release_answer(Some(ERR_SEC_SUCCESS)), Release::Yes);
        for status in [-67050, -67062, -67061, -1] {
            assert_eq!(release_answer(Some(status)), Release::No, "{status}");
        }
        assert_eq!(release_answer(None), Release::Unknown);
        let sec = security().expect("Security.framework");
        let here = Owned(std::ptr::null());
        assert_eq!(
            static_code_check(sec, &here, "not a requirement ("),
            None,
            "a requirement that doesn't compile gave a status"
        );
    }

    /// Failure: a lookup that was refused (a locked keychain, an item another signature made or planted, any answer but "no item") goes on to add one, so the app writes a second key beside a planted one, or over the person's own while their keychain is locked. Only "no item" leads to an add; every other status the probe has seen, and any other, is a refusal.
    #[test]
    fn only_no_item_leads_to_an_add() {
        assert_eq!(after_lookup(ERR_SEC_ITEM_NOT_FOUND), Lookup::Absent);
        assert_eq!(after_lookup(ERR_SEC_SUCCESS), Lookup::Found);
        for status in [-25293, ERR_SEC_INTERACTION_NOT_ALLOWED, -25244, ERR_SEC_DUPLICATE_ITEM, -128, -34018, -1, 1] {
            assert_eq!(after_lookup(status), Lookup::Refused(status), "{status} would go on to add an item");
        }
    }

    /// Failure: something else in the app turns the process-wide switch back on, or touches it at all, so a later Keychain call could prompt: it's set off once, here, and nowhere else names it (the Keychain's code is opened by name, so a call to it is a mention of it).
    #[test]
    fn the_switch_is_set_once_and_never_back() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut found = Vec::new();
        let mut stack = vec![crates.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("target") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    let text = text.split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
                    for line in text.lines().filter(|line| !line.trim_start().starts_with("//")) {
                        if line.contains("SecKeychainSetUserInteractionAllowed") {
                            found.push(format!("{}: {}", path.strip_prefix(crates).unwrap().display(), line.trim()));
                        }
                    }
                }
            }
        }
        assert_eq!(
            found,
            ["recto-platform/src/find_key_mac.rs: const SWITCH: &CStr = c\"SecKeychainSetUserInteractionAllowed\";"],
            "the switch is named somewhere new"
        );
    }

    /// Failure: the look or the add leaves the data-protection keychain or the app's access group out, so the item goes to the login keychain, where another program of this user's could plant or overwrite it (runs 43706807396 and 43708234469), or into another group; or the item is made readable before the first unlock, or carried to another Mac. Each dictionary is built as the app builds it, and nothing is asked of a keychain.
    #[test]
    fn the_look_and_the_add_name_the_data_protection_keychain_and_the_apps_group() {
        let sec = security().expect("Security.framework");
        let names = Names {
            service: cf_string("test-service").unwrap(),
            account: cf_string("test-account").unwrap(),
            group: cf_string("6GND8Z2C7Z.io.github.kkollsga.RectoPDF").unwrap(),
        };
        let bytes = [0u8; KEY_BYTES];
        // SAFETY: the bytes live for the call, and CF copies them.
        let data = Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), KEY_BYTES as isize) });
        let (query, item) = (lookup(sec, &names).unwrap(), new_item(sec, &names, &data).unwrap());
        // SAFETY: dictionaries and keys, all held; the values got are the dictionaries', not released here.
        let value = |dictionary: &Owned, key: CfRef| unsafe { CFDictionaryGetValue(dictionary.0, key) };
        for (what, dictionary) in [("the look", &query), ("the add", &item)] {
            // SAFETY: a framework constant.
            assert_eq!(
                value(dictionary, sec.keys.data_protection),
                unsafe { kCFBooleanTrue },
                "{what} isn't in the data-protection keychain"
            );
            assert_eq!(
                value(dictionary, sec.keys.access_group),
                names.group.0,
                "{what} isn't in the app's access group"
            );
            assert!(value(dictionary, sec.keys.synchronizable).is_null(), "{what} asks for the item to go to iCloud");
        }
        assert_eq!(
            value(&item, sec.keys.accessible),
            sec.keys.after_first_unlock_this_device,
            "the item isn't readable after the first unlock, on this Mac only"
        );
    }

    /// Failure: a program without the app's entitlement, a release built without its profile, is taken as one whose item can't be read just now, or the other way round; or "no item" in the group is taken as a refusal, so the key is never made.
    #[test]
    fn missing_the_entitlement_is_its_own_refusal() {
        assert_eq!(refusal(ERR_SEC_MISSING_ENTITLEMENT), Refused::NotEntitled);
        assert_eq!(Refused::NotEntitled.code(), "keychain_not_entitled");
        for status in [ERR_SEC_INTERACTION_NOT_ALLOWED, -25293, -128, -1] {
            assert_eq!(refusal(status), Refused::Unreadable(status), "{status}");
        }
        assert_eq!(
            after_lookup(ERR_SEC_MISSING_ENTITLEMENT),
            Lookup::Refused(ERR_SEC_MISSING_ENTITLEMENT),
            "a missing entitlement led to an add"
        );
    }

    /// What the check costs, by hand (`--ignored`), on a built app: `RECTO_CHECK_BUNDLE=<…/RectoPDF.app>`.
    #[test]
    #[ignore = "a measurement, by hand"]
    fn what_the_release_check_costs() {
        let bundle = std::path::PathBuf::from(std::env::var_os("RECTO_CHECK_BUNDLE").expect("RECTO_CHECK_BUNDLE"));
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let met = path_meets(&bundle, "always");
            println!("checked {} in {:?}: {met}", bundle.display(), started.elapsed());
        }
    }
}
