//! **Everything slow, before the app quits.** The download, its checksum, the unpacking, the signature, the team and Gatekeeper all happen here, on a thread, while the app's in use. So what's left for the installer is one quit and one launch. A release is hostile until it's been through this: the zip's a list of paths someone else chose.

use crate::fetch::{self, FetchError, MAX_ZIP_BYTES};
use crate::plan::{Files, beside_app, new_app_in};
use crate::process::tool;
use crate::zip::{self, ZipError};
use crate::rules::Latest;
use crate::source::{Source, Trust};
use crate::{APP_BUNDLE, APP_EXECUTABLE, BUNDLE_ID, HELPER};
use recto_platform::local;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CODESIGN: &str = "/usr/bin/codesign";
const SPCTL: &str = "/usr/sbin/spctl";
const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";

/// Longest a tool or the unpacking here may take.
const TOOL_LIMIT: Duration = Duration::from_secs(5 * 60);
const QUICK_LIMIT: Duration = Duration::from_secs(30);

/// Why it's not going ahead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageError {
    pub code: &'static str,
    /// To finish "RectoPDF couldn't update: …". Our own words.
    pub why: String,
    /// It won't get better by trying again: the release is wrong, not the moment.
    pub lasting: bool,
}

impl StageError {
    fn now(code: &'static str, why: impl Into<String>) -> Self {
        Self { code, why: why.into(), lasting: false }
    }

    fn lasting(code: &'static str, why: impl Into<String>) -> Self {
        Self { code, why: why.into(), lasting: true }
    }
}

/// What staging needs.
#[derive(Clone, Copy, Debug)]
pub struct Stage<'a> {
    pub source: &'a Source,
    pub latest: &'a Latest,
    pub current: &'a str,
    pub app: &'a Path,
    pub files: &'a Files,
    pub user_agent: &'a str,
}

/// Whether this app can be replaced where it is, asked before anything's downloaded. A read-only place or a translocated one (run straight from Downloads or a disk image) can't, and neither can a volume that doesn't do an atomic swap, which is what keeps a half-replaced app from ever being possible. It looks at the real bundle and its folder: a swap of the bundle needs write access to both, and a probe of two folders of its own proves nothing about either.
pub fn check_place(app: &Path) -> Result<PathBuf, StageError> {
    let move_it = "move RectoPDF to Applications first";
    let cant_write = "RectoPDF can't replace itself where it is, as it or its folder can't be written to";
    if app.file_name().is_none_or(|name| name != APP_BUNDLE) || !app.is_absolute() {
        return Err(StageError::now("not_an_app", "this copy isn't a RectoPDF.app"));
    }
    if app.to_string_lossy().contains("/AppTranslocation/") {
        return Err(StageError::now("translocated", move_it));
    }
    let beside = beside_app(app).ok_or_else(|| StageError::now("not_an_app", "this copy isn't in a folder"))?;
    if !app.join(HELPER).is_file() {
        return Err(StageError::now("no_installer", "this copy has no installer"));
    }
    if !local::can_be_replaced(app) || !app.parent().is_some_and(local::can_be_replaced) {
        return Err(StageError::now("not_writable", cant_write));
    }
    match std::fs::symlink_metadata(&beside) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(StageError::now("not_a_folder", "something else is where the update goes")),
        Err(_) => std::fs::create_dir(&beside).map_err(|_| StageError::now("not_writable", cant_write))?,
    }
    // Private, as what's waiting here is about to be believed. One made by an earlier version is put right.
    local::make_folder_private(&beside).map_err(|_| StageError::now("not_writable", cant_write))?;
    let probe = beside.join("probe");
    let _ = std::fs::remove_dir_all(&probe);
    let swaps = (|| {
        std::fs::create_dir_all(probe.join("a"))?;
        std::fs::create_dir_all(probe.join("b"))?;
        local::swap_paths(&probe.join("a"), &probe.join("b"))
    })();
    let _ = std::fs::remove_dir_all(&probe);
    swaps.map_err(|_| StageError::now("no_swap", "this disk can't replace an app in one step"))?;
    Ok(beside)
}

/// What's ready to swap in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub app: PathBuf,
    /// The signature's hash of what was verified.
    pub cdhash: String,
}

/// Downloads, checks and unpacks a release beside the app. Comes back with the new app, or with why not, and the running app hasn't noticed. A version already staged and whole isn't downloaded again.
pub fn prepare(stage: &Stage<'_>, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError> {
    let version = stage.latest.version.as_str();
    let beside = check_place(stage.app)?;
    let new_app = new_app_in(&beside);
    // A test's copy has a bundle id of its own, so nothing of the system's confuses it with the real app. Anything else has this one, whatever it's told.
    let own_id = if stage.source.mode == crate::source::Mode::Test { plist_value(stage.app, "CFBundleIdentifier").ok().flatten().unwrap_or_default() } else { BUNDLE_ID.to_owned() };
    let marker = beside.join("staged.json");
    let wanted = format!("{version} {}", stage.latest.sha256);
    if std::fs::read_to_string(&marker).is_ok_and(|text| text == wanted) && new_app.is_dir() {
        say("already downloaded");
        let cdhash = verify_app(&new_app, stage.source.trust, version, &own_id)?;
        return Ok(Staged { app: new_app, cdhash });
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(beside.join("new"));
    local::ensure_private_dir(&stage.files.work).map_err(|_| StageError::now("no_state", "the state folder can't be written"))?;
    let zip = stage.files.zip(version);
    say("downloading");
    let got = fetch::download(stage.source, &stage.source.zip_url(version), stage.user_agent, &zip, MAX_ZIP_BYTES, fetch::DOWNLOAD_LIMIT).map_err(|error| match error {
        FetchError::TooLarge => StageError::lasting("zip_too_large", "the release is larger than it could be"),
        // One 404 can be GitHub still catching up with a release just published. It's looked at again, by the back-off.
        FetchError::NotFound => StageError::now("zip_missing", "the release has no such file yet"),
        other => StageError::now("download_failed", format!("the download failed: {}", other.words())),
    })?;
    let unpacked = (|| {
        if got != stage.latest.sha256 {
            return Err(StageError::lasting("checksum", "the download isn't the file the release describes"));
        }
        say("checksum good, unpacking");
        unpack(&zip, &beside)
    })();
    let _ = std::fs::remove_file(&zip);
    unpacked?;
    say("verifying");
    let cdhash = verify_app(&new_app, stage.source.trust, version, &own_id).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(beside.join("new"));
    })?;
    std::fs::write(&marker, wanted).map_err(|_| StageError::now("not_writable", "the update's folder can't be written"))?;
    Ok(Staged { app: new_app, cdhash })
}

/// Unpacks into a private folder, by our own reader, and moves it into place once it's whole. A bad zip is the release's fault. A disk that's full or a read that fails isn't, and it might go better later.
fn unpack(zip_file: &Path, beside: &Path) -> Result<(), StageError> {
    let cant_write = |_| StageError::now("not_writable", "the update's folder can't be written");
    let unpacking = beside.join("unpacking");
    let _ = std::fs::remove_dir_all(&unpacking);
    std::fs::create_dir_all(&unpacking).map_err(cant_write)?;
    let result = (|| {
        zip::unpack(zip_file, APP_BUNDLE, &unpacking, std::time::Instant::now() + TOOL_LIMIT).map_err(|error| match error {
            ZipError::Bad(why) => StageError::lasting("zip_bad", format!("the release's zip is no good: {why}")),
            ZipError::TooSlow => StageError::now("unpack_slow", "unpacking took too long"),
            ZipError::Io(error) if error.kind() == std::io::ErrorKind::StorageFull => StageError::now("disk_full", "the disk is full"),
            ZipError::Io(_) => StageError::now("unpack_failed", "unpacking failed"),
        })?;
        let new_folder = beside.join("new");
        let _ = std::fs::remove_dir_all(&new_folder);
        std::fs::create_dir_all(&new_folder).map_err(cant_write)?;
        std::fs::rename(unpacking.join(APP_BUNDLE), new_folder.join(APP_BUNDLE)).map_err(cant_write)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&unpacking);
    result
}

/// How a system tool's question came out, keeping "no" apart from "it took too long or wouldn't start", which says nothing about the release.
enum Answer {
    Yes(Vec<u8>),
    No,
}

fn ask_tool(program: &str, args: &[&str], limit: Duration) -> Result<Answer, StageError> {
    let (fine, out, ran) = tool(program, args, limit, 1024 * 1024).map_err(|_| StageError::now("tool_missing", "a system tool is missing"))?;
    if ran.timed_out {
        return Err(StageError::now("tool_slow", "a system tool took too long"));
    }
    Ok(if fine { Answer::Yes(out) } else { Answer::No })
}

fn plist_value(app: &Path, key: &str) -> Result<Option<String>, StageError> {
    let plist = app.join("Contents/Info.plist");
    let command = format!("Print :{key}");
    let Some(path) = plist.to_str() else { return Ok(None) };
    Ok(match ask_tool(PLIST_BUDDY, &["-c", &command, path], QUICK_LIMIT)? {
        Answer::Yes(out) => Some(String::from_utf8_lossy(&out).trim().to_owned()),
        Answer::No => None,
    })
}

/// The version an app says it is.
#[must_use]
pub fn app_version(app: &Path) -> Option<String> {
    plist_value(app, "CFBundleShortVersionString").ok().flatten()
}

/// The Developer ID rule: an Apple-issued certificate, of this team.
fn developer_id_requirement(team: &str) -> String {
    format!("anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\"")
}

/// What the signature's hash of an app is, as `codesign` says it.
fn cdhash_of(app: &str) -> Result<String, StageError> {
    let (_, _, ran) = tool(CODESIGN, &["-d", "-vvv", app], QUICK_LIMIT, 64 * 1024).map_err(|_| StageError::now("tool_missing", "a system tool is missing"))?;
    if ran.timed_out {
        return Err(StageError::now("tool_slow", "a system tool took too long"));
    }
    let text = String::from_utf8_lossy(&ran.stderr);
    text.lines().find_map(|line| line.strip_prefix("CDHash=")).map(str::trim).filter(|hash| hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).map(str::to_owned).ok_or_else(|| StageError::lasting("signature", "the app's signature doesn't verify"))
}

/// That this is this app, this version, signed, by this team, and that Gatekeeper takes it, and what its signature's hash is. An app that isn't never gets as far as the running app quitting. A tool that takes too long says nothing about the release.
pub fn verify_app(app: &Path, trust: Trust, version: &str, bundle_id: &str) -> Result<String, StageError> {
    let path = app.to_str().ok_or_else(|| StageError::lasting("zip_bad", "the app's path can't be read"))?;
    if !std::fs::symlink_metadata(app).is_ok_and(|meta| meta.is_dir()) || !std::fs::symlink_metadata(app.join(APP_EXECUTABLE)).is_ok_and(|meta| meta.is_file()) {
        return Err(StageError::lasting("zip_bad", "the release's zip doesn't hold the app"));
    }
    if plist_value(app, "CFBundleIdentifier")?.as_deref() != Some(bundle_id) {
        return Err(StageError::lasting("wrong_app", "the release holds some other app"));
    }
    if plist_value(app, "CFBundleShortVersionString")?.as_deref() != Some(version) {
        return Err(StageError::lasting("wrong_version", "the release holds another version than it says"));
    }
    if matches!(ask_tool(CODESIGN, &["--verify", "--deep", "--strict", path], TOOL_LIMIT)?, Answer::No) {
        return Err(StageError::lasting("signature", "the app's signature doesn't verify"));
    }
    if let Trust::DeveloperId { team } = trust {
        let requirement = format!("={}", developer_id_requirement(team));
        if matches!(ask_tool(CODESIGN, &["--verify", "--deep", "--strict", "-R", &requirement, path], TOOL_LIMIT)?, Answer::No) {
            return Err(StageError::lasting("team", format!("the app isn't signed by RectoPDF's developer (team {team})")));
        }
        if matches!(ask_tool(SPCTL, &["--assess", "--type", "execute", path], TOOL_LIMIT)?, Answer::No) {
            return Err(StageError::lasting("gatekeeper", "macOS's Gatekeeper doesn't accept the app"));
        }
    }
    cdhash_of(path)
}

/// The installer's last look, once the old app has gone and with nothing swapped yet: that what's waiting is still the very thing that was verified, however long ago that was. Sound, and its signature's hash the one that was checked: the hash covers the program, its Info.plist with the version and bundle id, and the seal over every other file. A real build also has to be this team's. A tool that won't answer counts as no, as there's no more waiting to be done at this point.
#[must_use]
pub fn still_what_was_verified(app: &Path, cdhash: &str, team: Option<&str>) -> bool {
    let Some(path) = app.to_str() else { return false };
    let requirement = team.map(|team| format!("={}", developer_id_requirement(team)));
    let mut args = vec!["--verify", "--strict"];
    if let Some(requirement) = &requirement {
        args.extend(["-R", requirement.as_str()]);
    }
    args.push(path);
    let sound = tool(CODESIGN, &args, QUICK_LIMIT, 1024 * 1024).is_ok_and(|(fine, _, _)| fine);
    sound && cdhash_of(path).is_ok_and(|now| now.eq_ignore_ascii_case(cdhash))
}

/// Takes away what staging left, if it's for nothing now. Bounded: one folder of its own.
pub fn clear_staged(app: &Path) {
    if let Some(beside) = beside_app(app) {
        let _ = std::fs::remove_dir_all(beside.join("new"));
        let _ = std::fs::remove_file(beside.join("staged.json"));
        let _ = std::fs::remove_dir_all(beside.join("unpacking"));
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod tests {
    use super::*;
    use crate::fetch::tests::{Answer, Server};
    use crate::rules::parse_latest;
    use crate::source::resolve;
    use crate::TEAM_ID;
    use crate::zip::tests::{Made, make_zip};
    use sha2::Digest as _;
    use std::process::Command;

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-stage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small stand-in app, ad-hoc signed, as a release's own would be.
    pub(crate) fn fake_app(at: &Path, version: &str) -> PathBuf {
        let app = at.join(APP_BUNDLE);
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleExecutable</key><string>RectoPDF</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
        // Real programs, whose signatures live in the program as a release's do: a script's would be in file attributes, which a zip doesn't carry.
        std::fs::copy("/usr/bin/true", macos.join("RectoPDF")).unwrap();
        std::fs::copy("/usr/bin/true", macos.join("recto-updater")).unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&app).status().unwrap().success());
        app
    }

    pub(crate) fn zip_of(app: &Path, to: &Path) -> (Vec<u8>, String) {
        assert!(Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(app).arg(to).status().unwrap().success());
        let bytes = std::fs::read(to).unwrap();
        let sha = sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        (bytes, sha)
    }

    pub(crate) fn latest_json(version: &str, sha: &str) -> Vec<u8> {
        format!(r#"{{"version":"{version}","tag":"v{version}","minimum_macos":"15.0","zip":"RectoPDF-{version}.zip","sha256":"{sha}"}}"#).into_bytes()
    }

    /// An installed "old" app, a server with a release on it, and state, all in a temp folder.
    struct Bed {
        dir: PathBuf,
        server: Server,
        source: Source,
        files: Files,
        app: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let dir = temp_dir(name);
            let root = dir.join("state");
            local::ensure_private_dir(&root).unwrap();
            let install = dir.join("install");
            std::fs::create_dir_all(&install).unwrap();
            let app = fake_app(&install, "0.1.0");
            let server = Server::start();
            let source = server.source();
            Self { server, source, files: Files::in_state(&root), app, dir }
        }

        fn release(&self, version: &str, inner: &str) -> Latest {
            let built = self.dir.join(format!("built-{version}-{inner}"));
            std::fs::create_dir_all(&built).unwrap();
            let app = fake_app(&built, inner);
            let (bytes, sha) = zip_of(&app, &built.join("r.zip"));
            self.server.set(&format!("/releases/download/v{version}/RectoPDF-{version}.zip"), Answer::Body(bytes));
            parse_latest(&latest_json(version, &sha)).unwrap()
        }

        fn stage(&self, latest: &Latest, source: &Source) -> Result<Staged, StageError> {
            let stage = Stage { source, latest, current: "0.1.0", app: &self.app, files: &self.files, user_agent: "RectoPDF/0.1.0" };
            prepare(&stage, &mut |_| {})
        }

        /// What must be true of any refusal: the running app's where it was, and nothing's staged.
        fn nothing_changed(&self) {
            assert_eq!(app_version(&self.app).as_deref(), Some("0.1.0"));
            assert!(!new_app_in(&beside_app(&self.app).unwrap()).exists(), "a refused release stays out");
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Failure: a good release doesn't get staged, or it's downloaded twice when the first copy's still good.
    #[test]
    fn a_good_release_is_ready_beside_the_app() {
        let bed = Bed::new("good");
        let latest = bed.release("0.2.0", "0.2.0");
        let staged = bed.stage(&latest, &bed.source).unwrap();
        assert_eq!(staged.app, new_app_in(&beside_app(&bed.app).unwrap()));
        assert_eq!(app_version(&staged.app).as_deref(), Some("0.2.0"));
        assert_eq!(staged.cdhash.len(), 40, "{staged:?}");
        // Nobody else can get at what's waiting, or at what's about to be the app.
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(std::fs::metadata(beside_app(&bed.app).unwrap()).unwrap().mode() & 0o777, 0o700);
        assert!(still_what_was_verified(&staged.app, &staged.cdhash, None), "what was checked is what's there");
        assert_eq!(app_version(&bed.app).as_deref(), Some("0.1.0"), "the running app isn't touched");
        assert!(!bed.files.zip("0.2.0").exists(), "the zip goes once it's unpacked");
        let asked = bed.server.requests.lock().unwrap().len();
        bed.stage(&latest, &bed.source).unwrap();
        assert_eq!(bed.server.requests.lock().unwrap().len(), asked, "not fetched again");
    }

    /// Failure: a download that isn't what the release says gets installed.
    #[test]
    fn a_wrong_checksum_is_refused() {
        let bed = Bed::new("checksum");
        let mut latest = bed.release("0.2.0", "0.2.0");
        latest.sha256 = "0".repeat(64);
        let error = bed.stage(&latest, &bed.source).unwrap_err();
        assert_eq!((error.code, error.lasting), ("checksum", true));
        bed.nothing_changed();
        assert!(!bed.files.zip("0.2.0").exists());
    }

    /// Failure: a damaged zip whose checksum is right, or one holding paths outside the app or another app altogether, gets as far as being installed.
    #[test]
    fn a_corrupt_or_foreign_zip_is_refused() {
        let bed = Bed::new("corrupt");
        // Bytes that aren't a zip, with a checksum to match.
        let junk = b"this isn't a zip".to_vec();
        let sha: String = sha2::Sha256::digest(&junk).iter().map(|byte| format!("{byte:02x}")).collect();
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(junk));
        let latest = parse_latest(&latest_json("0.2.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "zip_bad");
        bed.nothing_changed();
        // A zip of something else altogether.
        let other = bed.dir.join("other");
        std::fs::create_dir_all(other.join("Evil.app/Contents")).unwrap();
        std::fs::write(other.join("Evil.app/Contents/x"), "x").unwrap();
        let (bytes, sha) = zip_of(&other.join("Evil.app"), &other.join("o.zip"));
        bed.server.set("/releases/download/v0.3.0/RectoPDF-0.3.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.3.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "zip_bad");
        bed.nothing_changed();
    }

    /// Failure: a release holding an older version than it claims gets in, which would let an old signed build be replayed as a new one.
    #[test]
    fn a_release_holding_another_version_is_refused() {
        let bed = Bed::new("replay");
        let latest = bed.release("0.2.0", "0.0.9");
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "wrong_version");
        bed.nothing_changed();
    }

    /// Failure: a release holding some other app, signed and all, gets in. Even a real one of this team's would be another app, and take this one's place.
    #[test]
    fn a_release_holding_another_app_is_refused() {
        let bed = Bed::new("another");
        let built = bed.dir.join("other-app");
        std::fs::create_dir_all(&built).unwrap();
        let app = fake_app(&built, "0.2.0");
        let plist = app.join("Contents/Info.plist");
        let text = std::fs::read_to_string(&plist).unwrap().replace(BUNDLE_ID, "com.evil.App");
        std::fs::write(&plist, text).unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&app).status().unwrap().success());
        let (bytes, sha) = zip_of(&app, &built.join("o.zip"));
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.2.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "wrong_app");
        bed.nothing_changed();
    }

    /// Failure: a link inside the bundle reaches out of it, so the signature's vouching for something that can change after it's been checked.
    #[test]
    fn a_link_out_of_the_bundle_is_refused() {
        let bed = Bed::new("link");
        let built = bed.dir.join("link-app");
        std::fs::create_dir_all(&built).unwrap();
        let app = fake_app(&built, "0.2.0");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", app.join("Contents/Resources/evil")).unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&app).status().unwrap().success());
        let (bytes, sha) = zip_of(&app, &built.join("l.zip"));
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.2.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "zip_bad", "the reader doesn't take a link at all");
        bed.nothing_changed();
    }

    /// Failure: an ad-hoc-signed or unsigned app gets past the real rule. **This is the refusal, and all that can be tested without a real signed release:** the Developer ID requirement and Gatekeeper passing on a good one stay unproven until there is one. Holds every route: unsigned, ad hoc, and a tampered bundle.
    #[test]
    fn the_real_rule_refuses_what_a_test_build_is() {
        let bed = Bed::new("refuse");
        let latest = bed.release("0.2.0", "0.2.0");
        let real = resolve(false, true, Some(bed.server.url.clone()), Some("developer-id".into()));
        assert_eq!(real.trust, Trust::DeveloperId { team: TEAM_ID });
        let error = bed.stage(&latest, &real).unwrap_err();
        assert_eq!((error.code, error.lasting), ("team", true), "{error:?}");
        bed.nothing_changed();

        // With no signature at all, it doesn't get as far as the team.
        let unsigned = bed.dir.join("unsigned");
        std::fs::create_dir_all(&unsigned).unwrap();
        let app = fake_app(&unsigned, "0.3.0");
        assert!(Command::new(CODESIGN).args(["--remove-signature"]).arg(&app).status().unwrap().success());
        let (bytes, sha) = zip_of(&app, &unsigned.join("u.zip"));
        bed.server.set("/releases/download/v0.3.0/RectoPDF-0.3.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.3.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "signature", "not even a test's rule takes one");
        assert_eq!(bed.stage(&latest, &real).unwrap_err().code, "signature");
        bed.nothing_changed();

        // Past the signature: a bundle changed after it was signed fails even a test run's rule.
        let built = bed.dir.join("tampered");
        std::fs::create_dir_all(&built).unwrap();
        let app = fake_app(&built, "0.4.0");
        let mut changed = std::fs::read(app.join("Contents/MacOS/RectoPDF")).unwrap();
        // In its code, as the padding at the end of a signature isn't covered by anything.
        changed[0x1000] ^= 0xFF;
        std::fs::write(app.join("Contents/MacOS/RectoPDF"), changed).unwrap();
        let (bytes, sha) = zip_of(&app, &built.join("t.zip"));
        bed.server.set("/releases/download/v0.4.0/RectoPDF-0.4.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.4.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "signature");
        bed.nothing_changed();
    }

    /// Failure: what's swapped in isn't what was verified. Everything slow comes first, and a by-hand install can wait ten minutes after that with something in flight, so what's waiting gets looked at once more at the last moment. **The review got an unsigned 9.9.9 swapped in and run this way.** A changed file, a changed version, another build altogether and no signature are all refused, and the real rule on top refuses an ad-hoc build.
    #[test]
    fn what_was_verified_is_what_the_installer_will_take() {
        let bed = Bed::new("pin");
        let latest = bed.release("0.2.0", "0.2.0");
        let staged = bed.stage(&latest, &bed.source).unwrap();
        assert!(still_what_was_verified(&staged.app, &staged.cdhash, None));
        assert!(!still_what_was_verified(&staged.app, &staged.cdhash, Some(TEAM_ID)), "an ad-hoc build isn't this team's");
        assert!(!still_what_was_verified(&staged.app, &"0".repeat(40), None), "another hash");
        let executable = staged.app.join("Contents/MacOS/RectoPDF");
        let original = std::fs::read(&executable).unwrap();
        std::fs::write(&executable, b"changed").unwrap();
        assert!(!still_what_was_verified(&staged.app, &staged.cdhash, None), "a changed executable");
        std::fs::write(&executable, &original).unwrap();
        assert!(still_what_was_verified(&staged.app, &staged.cdhash, None), "put back, it's the same bytes");
        let plist = staged.app.join("Contents/Info.plist");
        let text = std::fs::read_to_string(&plist).unwrap();
        std::fs::write(&plist, text.replace("0.2.0", "9.9.9")).unwrap();
        assert!(!still_what_was_verified(&staged.app, &staged.cdhash, None), "another version in the same bundle");
        std::fs::write(&plist, &text).unwrap();
        std::fs::write(staged.app.join("Contents/extra"), "x").unwrap();
        assert!(!still_what_was_verified(&staged.app, &staged.cdhash, None), "something added");
        let swapped_in = bed.dir.join("other-build");
        std::fs::create_dir_all(&swapped_in).unwrap();
        let other = fake_app(&swapped_in, "0.2.0");
        std::fs::write(other.join("Contents/Resources-extra"), "not what was verified").unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&other).status().unwrap().success());
        assert!(!still_what_was_verified(&other, &staged.cdhash, None), "a build that's good in itself and isn't the one");
        assert!(Command::new(CODESIGN).args(["--remove-signature"]).arg(&other).status().unwrap().success());
        assert!(!still_what_was_verified(&other, &staged.cdhash, None), "no signature");
    }

    /// Failure: a real zip isn't checked by the reader that unpacks it, so a zip the system's own tools would treat differently gets through. Through the whole of staging, with a checksum to match: a link, a name that leaves, and something not listed. **And the modes are ours:** a re-zipped real build at 0777 would have left the installed app writable by anyone.
    #[test]
    fn a_hostile_zip_goes_no_further_than_the_reader() {
        let bed = Bed::new("hostile");
        let release = |zip: Vec<u8>| {
            let sha: String = sha2::Sha256::digest(&zip).iter().map(|byte| format!("{byte:02x}")).collect();
            bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(zip));
            parse_latest(&latest_json("0.2.0", &sha)).unwrap()
        };
        let outside = bed.dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let mut leaving = Made::file("RectoPDF.app/aaaaaaaaaaaaaaa", b"x");
        leaving.local_name = format!("{}/escape", outside.display()).into_bytes();
        let cases: Vec<Vec<u8>> = vec![
            make_zip(&[leaving]),
            make_zip(&[Made { mode: 0o120_777, ..Made::file("RectoPDF.app/link", outside.to_str().unwrap().as_bytes()) }, Made::file("RectoPDF.app/link/pwned", b"p")]),
            make_zip(&[Made::file("RectoPDF.app/x", b"x"), Made { unlisted: true, ..Made::file("RectoPDF.app/unlisted", b"y") }]),
            make_zip(&[Made { deflate: true, claimed_size: Some(1024), ..Made::file("RectoPDF.app/bomb", &vec![0u8; 20 * 1024 * 1024]) }]),
        ];
        for zip in cases {
            let error = bed.stage(&release(zip), &bed.source).unwrap_err();
            assert_eq!((error.code, error.lasting), ("zip_bad", true), "{error:?}");
            bed.nothing_changed();
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing's gone anywhere");
            assert!(!beside_app(&bed.app).unwrap().join("unpacking").exists());
        }
        // A genuine build, re-zipped with everything open to everyone.
        let built = bed.dir.join("open");
        std::fs::create_dir_all(&built).unwrap();
        let app = fake_app(&built, "0.2.0");
        assert!(Command::new("/bin/chmod").args(["-R", "777"]).arg(&app).status().unwrap().success());
        let (bytes, sha) = zip_of(&app, &built.join("o.zip"));
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(bytes));
        let staged = bed.stage(&parse_latest(&latest_json("0.2.0", &sha)).unwrap(), &bed.source).unwrap();
        fn walk(path: &Path, seen: &mut Vec<(PathBuf, u32)>) {
            use std::os::unix::fs::MetadataExt as _;
            seen.push((path.to_path_buf(), std::fs::symlink_metadata(path).unwrap().mode() & 0o7777));
            if path.is_dir() {
                for entry in std::fs::read_dir(path).unwrap().flatten() {
                    walk(&entry.path(), seen);
                }
            }
        }
        let mut seen = Vec::new();
        walk(&staged.app, &mut seen);
        assert!(seen.len() > 3);
        for (path, mode) in seen {
            assert_eq!(mode & 0o7022, 0, "{} is {mode:o}", path.display());
        }
    }

    /// Failure: something that may well pass counts as the release's fault, so that version's never installed by itself again and nothing's said. A 404 in the moments after publishing, a tool that takes too long and a disk that's full all get tried again. A bad file doesn't.
    #[test]
    fn what_may_pass_isnt_held_against_the_release() {
        let bed = Bed::new("transient");
        let latest = parse_latest(&latest_json("0.2.0", &"a".repeat(64))).unwrap();
        let error = bed.stage(&latest, &bed.source).unwrap_err();
        assert_eq!((error.code, error.lasting), ("zip_missing", false), "a 404");
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Status(503));
        let error = bed.stage(&latest, &bed.source).unwrap_err();
        assert_eq!((error.code, error.lasting), ("download_failed", false), "a server error");
        // The folder it unpacks in can't be written. Not the zip's fault, whatever it holds.
        let latest = bed.release("0.2.0", "0.2.0");
        let beside = beside_app(&bed.app).unwrap();
        std::fs::create_dir_all(&beside).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o500)).unwrap();
        let error = unpack(&{
            let zip = bed.dir.join("z.zip");
            std::fs::write(&zip, make_zip(&[Made::file("RectoPDF.app/x", b"x")])).unwrap();
            zip
        }, &beside);
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(error.is_err_and(|error| !error.lasting), "a folder it can't write to");
        // A tool that won't answer in time.
        assert!(matches!(ask_tool("/bin/sleep", &["30"], Duration::from_millis(50)), Err(StageError { code: "tool_slow", lasting: false, .. })));
        assert_eq!(bed.stage(&latest, &bed.source).map(|staged| app_version(&staged.app)).unwrap(), Some("0.2.0".to_owned()), "and it goes through when it can");
    }

    /// Failure: an app with no way to replace itself downloads anything before saying so, or the check itself leaves anything lying about.
    #[test]
    fn a_place_that_cant_be_replaced_says_so_first() {
        let dir = temp_dir("place");
        assert_eq!(check_place(Path::new("relative/RectoPDF.app")).unwrap_err().code, "not_an_app");
        assert_eq!(check_place(&dir.join("Other.app")).unwrap_err().code, "not_an_app");
        assert_eq!(check_place(Path::new("/private/var/folders/x/AppTranslocation/y/d/RectoPDF.app")).unwrap_err().code, "translocated");
        assert_eq!(check_place(&dir.join("RectoPDF.app")).unwrap_err().code, "no_installer");
        let app = fake_app(&dir, "0.1.0");
        let beside = check_place(&app).unwrap();
        assert!(!beside.join("probe").exists());
        std::fs::remove_dir_all(&beside).unwrap();
        std::fs::write(&beside, "in the way").unwrap();
        assert_eq!(check_place(&app).unwrap_err().code, "not_a_folder");
        std::fs::remove_file(&beside).unwrap();
        // **The review's way in:** the probe swap of two folders of its own passed with the bundle read-only, and the real swap then failed with the app already gone, every day for ever. The real bundle and its real folder.
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o555)).unwrap();
        let refused = check_place(&app).unwrap_err();
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(refused.code, "not_writable");
        assert!(refused.why.contains("can't replace itself"), "{refused:?}");
        assert!(!beside.exists(), "and nothing's been made for it");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let refused = check_place(&app).unwrap_err();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(refused.code, "not_writable", "the folder it's in");
        assert!(Command::new("/usr/bin/chflags").arg("uchg").arg(&app).status().unwrap().success());
        let refused = check_place(&app).unwrap_err();
        assert!(Command::new("/usr/bin/chflags").arg("nouchg").arg(&app).status().unwrap().success());
        assert_eq!(refused.code, "not_writable", "a bundle locked in the Finder");
        // One made by an earlier version, open to others, is put right.
        std::fs::create_dir(&beside).unwrap();
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755)).unwrap();
        check_place(&app).unwrap();
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(std::fs::metadata(&beside).unwrap().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
