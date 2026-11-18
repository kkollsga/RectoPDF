//! Replacement metadata must preserve both a download's origin and the source's access controls.

#![cfg(target_os = "macos")]

use recto_platform::local;
use std::process::Command;

/// Failure: copying the ACL changes a download's quarantine origin, or restoring attributes afterward loses them under an explicit deny-writeextattr ACL.
#[test]
fn replacement_attributes_keep_the_download_origin_and_enforce_the_original_acl() {
    let dir = recto_testpdf::TempDir::new("recto-replacement-attributes-");
    let quarantine = b"0083;5f000000;Safari;";
    for restricted in [false, true] {
        let source = dir.join(format!("source-{restricted}.pdf"));
        let target = dir.join(format!("target-{restricted}.pdf"));
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&target, b"replacement").unwrap();
        local::set_attribute(&source, "com.apple.quarantine", quarantine).unwrap();
        local::set_attribute(&source, "com.example.tag", b"preserved").unwrap();
        if restricted {
            let changed = Command::new("/bin/chmod")
                .args(["+a", "everyone deny writeextattr"])
                .arg(&source)
                .output()
                .unwrap();
            assert!(
                changed.status.success(),
                "couldn't make the ACL fixture: {}",
                String::from_utf8_lossy(&changed.stderr)
            );
        }
        assert_eq!(local::carry_attributes(&source, &target).unwrap(), 0, "restricted={restricted}");
        assert_eq!(
            local::attribute(&target, "com.apple.quarantine").as_deref(),
            Some(quarantine.as_slice()),
            "restricted={restricted}"
        );
        assert_eq!(
            local::attribute(&target, "com.example.tag").as_deref(),
            Some(b"preserved".as_slice()),
            "restricted={restricted}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement", "metadata copying never changes file bytes");
        let changed = local::set_attribute(&target, "com.example.new", b"new");
        if restricted {
            assert_eq!(
                changed.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
                "the actual deny ACL must still apply"
            );
        } else {
            changed.unwrap();
        }
    }
}
