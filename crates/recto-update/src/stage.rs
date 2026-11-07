//! **Everything slow, before the app quits.** The download, its checksum, the unpacking, the signature, the team and Gatekeeper all happen here, on a thread, while the app's in use. So what's left for the installer is one quit and one launch. A release is hostile until it's been through this: the zip's a list of paths someone else chose.

use crate::fetch::{self, FetchError, MAX_ZIP_BYTES};
use crate::plan::{Files, beside_app, new_app_in};
use crate::process::tool;
use crate::rules::Latest;
use crate::source::{Source, Trust};
use crate::{APP_BUNDLE, APP_EXECUTABLE, BUNDLE_ID, HELPER};
use recto_platform::local;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DITTO: &str = "/usr/bin/ditto";
const UNZIP: &str = "/usr/bin/unzip";
const CODESIGN: &str = "/usr/bin/codesign";
const SPCTL: &str = "/usr/sbin/spctl";
const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";

/// Longest a tool here may take.
const TOOL_LIMIT: Duration = Duration::from_secs(5 * 60);
const QUICK_LIMIT: Duration = Duration::from_secs(30);
/// Most files a release's zip may have. The app has a handful.
const MAX_ZIP_ENTRIES: usize = 20_000;
/// Most it may unpack to, by its own account.
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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

/// Whether this app can be replaced where it is, asked before anything's downloaded. A read-only place or a translocated one (run straight from Downloads or a disk image) can't, and neither can a volume that doesn't do an atomic swap, which is what keeps a half-replaced app from ever being possible.
pub fn check_place(app: &Path) -> Result<PathBuf, StageError> {
    let move_it = "move RectoPDF to Applications first";
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
    match std::fs::symlink_metadata(&beside) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(StageError::now("not_a_folder", "something else is where the update goes")),
        Err(_) => std::fs::create_dir(&beside).map_err(|_| StageError::now("not_writable", "RectoPDF can't write to the folder it's in"))?,
    }
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

/// Downloads, checks and unpacks a release beside the app. Comes back with the new app, or with why not, and the running app hasn't noticed. A version already staged and whole isn't downloaded again.
pub fn prepare(stage: &Stage<'_>, say: &mut dyn FnMut(&str)) -> Result<PathBuf, StageError> {
    let version = stage.latest.version.as_str();
    let beside = check_place(stage.app)?;
    let new_app = new_app_in(&beside);
    // A test's copy has a bundle id of its own, so nothing of the system's confuses it with the real app. Anything else has this one, whatever it's told.
    let own_id = if stage.source.mode == crate::source::Mode::Test { plist_value(stage.app, "CFBundleIdentifier").unwrap_or_default() } else { BUNDLE_ID.to_owned() };
    let marker = beside.join("staged.json");
    let wanted = format!("{version} {}", stage.latest.sha256);
    if std::fs::read_to_string(&marker).is_ok_and(|text| text == wanted) && new_app.is_dir() {
        say("already downloaded");
        verify_app(&new_app, stage.source.trust, version, &own_id)?;
        return Ok(new_app);
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(beside.join("new"));
    local::ensure_private_dir(&stage.files.work).map_err(|_| StageError::now("no_state", "the state folder can't be written"))?;
    let zip = stage.files.zip(version);
    say("downloading");
    let got = fetch::download(stage.source, &stage.source.zip_url(version), stage.user_agent, &zip, MAX_ZIP_BYTES, fetch::DOWNLOAD_LIMIT).map_err(|error| match error {
        FetchError::TooLarge => StageError::lasting("zip_too_large", "the release is larger than it could be"),
        FetchError::NotFound => StageError::lasting("zip_missing", "the release has no such file"),
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
    verify_app(&new_app, stage.source.trust, version, &own_id).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(beside.join("new"));
    })?;
    std::fs::write(&marker, wanted).map_err(|_| StageError::now("not_writable", "the update's folder can't be written"))?;
    Ok(new_app)
}

/// Whether a path in a zip stays inside the zip's own folder and is part of the app.
fn entry_is_ours(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.chars().any(char::is_control) {
        return false;
    }
    let mut parts = name.split('/');
    let first = parts.next().unwrap_or("");
    (first == APP_BUNDLE || first == "__MACOSX") && !name.split('/').any(|part| part == ".." || part == ".")
}

/// What the zip says it holds, without believing it: every path has to be the app's, and its total has to be sane.
fn check_zip(zip: &Path) -> Result<(), StageError> {
    let bad = || StageError::lasting("zip_bad", "the release's zip is damaged or isn't an app");
    let path = zip.to_str().ok_or_else(bad)?;
    let (fine, listed, _) = tool(UNZIP, &["-Z1", path], QUICK_LIMIT, 8 * 1024 * 1024).map_err(|_| StageError::now("tool_missing", "a system tool is missing"))?;
    if !fine {
        return Err(bad());
    }
    let text = String::from_utf8(listed).map_err(|_| bad())?;
    let mut has_app = false;
    for (count, name) in text.lines().enumerate() {
        if count >= MAX_ZIP_ENTRIES || !entry_is_ours(name) {
            return Err(StageError::lasting("zip_bad", "the release's zip has something in it that isn't the app"));
        }
        has_app |= name.starts_with(APP_BUNDLE);
    }
    if !has_app {
        return Err(bad());
    }
    let (fine, summary, _) = tool(UNZIP, &["-Zt", path], QUICK_LIMIT, 4096).map_err(|_| StageError::now("tool_missing", "a system tool is missing"))?;
    if !fine {
        return Err(bad());
    }
    let summary = String::from_utf8_lossy(&summary);
    let total = summary.split(',').find_map(|part| part.trim().strip_suffix("bytes uncompressed").and_then(|number| number.trim().parse::<u64>().ok()));
    if total.is_none_or(|total| total > MAX_UNPACKED_BYTES) {
        return Err(StageError::lasting("zip_bad", "the release's zip is larger than the app could be"));
    }
    Ok(())
}

fn unpack(zip: &Path, beside: &Path) -> Result<(), StageError> {
    check_zip(zip)?;
    let unpacking = beside.join("unpacking");
    let _ = std::fs::remove_dir_all(&unpacking);
    std::fs::create_dir_all(&unpacking).map_err(|_| StageError::now("not_writable", "the update's folder can't be written"))?;
    let result = (|| {
        let bad = || StageError::lasting("zip_bad", "the release's zip is damaged or isn't an app");
        let (zip_path, into) = (zip.to_str().ok_or_else(bad)?, unpacking.to_str().ok_or_else(bad)?);
        let (fine, _, ran) = tool(DITTO, &["-x", "-k", zip_path, into], TOOL_LIMIT, 1024 * 1024).map_err(|_| StageError::now("tool_missing", "a system tool is missing"))?;
        if ran.timed_out {
            return Err(StageError::now("unpack_slow", "unpacking took too long"));
        }
        if !fine {
            return Err(bad());
        }
        let unpacked = unpacking.join(APP_BUNDLE);
        // Not a link to somewhere else, whatever it's called.
        if !std::fs::symlink_metadata(&unpacked).is_ok_and(|meta| meta.is_dir()) {
            return Err(bad());
        }
        let new_folder = beside.join("new");
        let _ = std::fs::remove_dir_all(&new_folder);
        std::fs::create_dir_all(&new_folder).map_err(|_| StageError::now("not_writable", "the update's folder can't be written"))?;
        std::fs::rename(&unpacked, new_folder.join(APP_BUNDLE)).map_err(|_| StageError::now("not_writable", "the update's folder can't be written"))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&unpacking);
    result
}

fn plist_value(app: &Path, key: &str) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let command = format!("Print :{key}");
    let (fine, out, _) = tool(PLIST_BUDDY, &["-c", &command, plist.to_str()?], QUICK_LIMIT, 4096).ok()?;
    fine.then(|| String::from_utf8_lossy(&out).trim().to_owned())
}

/// The version an app says it is.
#[must_use]
pub fn app_version(app: &Path) -> Option<String> {
    plist_value(app, "CFBundleShortVersionString")
}

/// The Developer ID rule: an Apple-issued certificate, of this team.
fn developer_id_requirement(team: &str) -> String {
    format!("=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\"")
}

/// That this is this app, this version, signed, by this team, and that Gatekeeper takes it. An app that isn't never gets as far as the running app quitting.
pub fn verify_app(app: &Path, trust: Trust, version: &str, bundle_id: &str) -> Result<(), StageError> {
    let path = app.to_str().ok_or_else(|| StageError::lasting("zip_bad", "the app's path can't be read"))?;
    if !std::fs::symlink_metadata(app).is_ok_and(|meta| meta.is_dir()) || !std::fs::symlink_metadata(app.join(APP_EXECUTABLE)).is_ok_and(|meta| meta.is_file()) {
        return Err(StageError::lasting("zip_bad", "the release's zip doesn't hold the app"));
    }
    if plist_value(app, "CFBundleIdentifier").as_deref() != Some(bundle_id) {
        return Err(StageError::lasting("wrong_app", "the release holds some other app"));
    }
    if app_version(app).as_deref() != Some(version) {
        return Err(StageError::lasting("wrong_version", "the release holds another version than it says"));
    }
    let no_tool = |_| StageError::now("tool_missing", "a system tool is missing");
    let (signed, _, _) = tool(CODESIGN, &["--verify", "--deep", "--strict", path], TOOL_LIMIT, 1024 * 1024).map_err(no_tool)?;
    if !signed {
        return Err(StageError::lasting("signature", "the app's signature doesn't verify"));
    }
    if let Trust::DeveloperId { team } = trust {
        let requirement = developer_id_requirement(team);
        let (ours, _, _) = tool(CODESIGN, &["--verify", "--deep", "--strict", "-R", &requirement, path], TOOL_LIMIT, 1024 * 1024).map_err(no_tool)?;
        if !ours {
            return Err(StageError::lasting("team", format!("the app isn't signed by RectoPDF's developer (team {team})")));
        }
        let (accepted, _, _) = tool(SPCTL, &["--assess", "--type", "execute", path], TOOL_LIMIT, 1024 * 1024).map_err(no_tool)?;
        if !accepted {
            return Err(StageError::lasting("gatekeeper", "macOS's Gatekeeper doesn't accept the app"));
        }
    }
    Ok(())
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
    use sha2::Digest as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-stage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small stand-in app, ad-hoc signed, as a release's own would be.
    pub(crate) fn fake_app(at: &Path, version: &str, script: &str) -> PathBuf {
        let app = at.join(APP_BUNDLE);
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleExecutable</key><string>RectoPDF</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
        let executable = macos.join("RectoPDF");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(macos.join("recto-updater"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(macos.join("recto-updater"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&app).status().unwrap().success());
        app
    }

    pub(crate) fn zip_of(app: &Path, to: &Path) -> (Vec<u8>, String) {
        assert!(Command::new(DITTO).args(["-c", "-k", "--keepParent"]).arg(app).arg(to).status().unwrap().success());
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
            let app = fake_app(&install, "0.1.0", "exit 0");
            let server = Server::start();
            let source = server.source();
            Self { server, source, files: Files::in_state(&root), app, dir }
        }

        fn release(&self, version: &str, inner: &str) -> Latest {
            let built = self.dir.join(format!("built-{version}-{inner}"));
            std::fs::create_dir_all(&built).unwrap();
            let app = fake_app(&built, inner, "exit 0");
            let (bytes, sha) = zip_of(&app, &built.join("r.zip"));
            self.server.set(&format!("/releases/download/v{version}/RectoPDF-{version}.zip"), Answer::Body(bytes));
            parse_latest(&latest_json(version, &sha)).unwrap()
        }

        fn stage(&self, latest: &Latest, source: &Source) -> Result<PathBuf, StageError> {
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
        assert_eq!(staged, new_app_in(&beside_app(&bed.app).unwrap()));
        assert_eq!(app_version(&staged).as_deref(), Some("0.2.0"));
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

    /// Failure: a path in a zip could land outside the folder it's unpacked into. These are checked before anything's unpacked.
    #[test]
    fn a_zips_paths_have_to_be_the_apps() {
        for fine in ["RectoPDF.app/", "RectoPDF.app/Contents/MacOS/RectoPDF", "__MACOSX/RectoPDF.app/._x", "RectoPDF.app/Contents/a..b"] {
            assert!(entry_is_ours(fine), "{fine}");
        }
        for wrong in ["", "/etc/passwd", "../x", "RectoPDF.app/../../x", "RectoPDF.app/./x", "Other.app/x", "RectoPDF.app.evil/x", "RectoPDF.app\\..\\x", "RectoPDF.app/a\u{1}b", "x"] {
            assert!(!entry_is_ours(wrong), "{wrong}");
        }
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
        let app = fake_app(&built, "0.2.0", "exit 0");
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
        let app = fake_app(&built, "0.2.0", "exit 0");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", app.join("Contents/Resources/evil")).unwrap();
        assert!(Command::new(CODESIGN).args(["--force", "--deep", "--sign", "-"]).arg(&app).status().unwrap().success());
        let (bytes, sha) = zip_of(&app, &built.join("l.zip"));
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.2.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "signature");
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

        // Past the signature: a bundle changed after it was signed fails even a test run's rule.
        let built = bed.dir.join("tampered");
        std::fs::create_dir_all(&built).unwrap();
        let app = fake_app(&built, "0.4.0", "exit 0");
        std::fs::write(app.join("Contents/MacOS/RectoPDF"), "#!/bin/sh\necho changed\n").unwrap();
        let (bytes, sha) = zip_of(&app, &built.join("t.zip"));
        bed.server.set("/releases/download/v0.4.0/RectoPDF-0.4.0.zip", Answer::Body(bytes));
        let latest = parse_latest(&latest_json("0.4.0", &sha)).unwrap();
        assert_eq!(bed.stage(&latest, &bed.source).unwrap_err().code, "signature");
        bed.nothing_changed();
    }

    /// Failure: an app with no way to replace itself downloads anything before saying so, or the check itself leaves anything lying about.
    #[test]
    fn a_place_that_cant_be_replaced_says_so_first() {
        let dir = temp_dir("place");
        assert_eq!(check_place(Path::new("relative/RectoPDF.app")).unwrap_err().code, "not_an_app");
        assert_eq!(check_place(&dir.join("Other.app")).unwrap_err().code, "not_an_app");
        assert_eq!(check_place(Path::new("/private/var/folders/x/AppTranslocation/y/d/RectoPDF.app")).unwrap_err().code, "translocated");
        assert_eq!(check_place(&dir.join("RectoPDF.app")).unwrap_err().code, "no_installer");
        let app = fake_app(&dir, "0.1.0", "exit 0");
        let beside = check_place(&app).unwrap();
        assert!(!beside.join("probe").exists());
        std::fs::remove_dir_all(&beside).unwrap();
        std::fs::write(&beside, "in the way").unwrap();
        assert_eq!(check_place(&app).unwrap_err().code, "not_a_folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
