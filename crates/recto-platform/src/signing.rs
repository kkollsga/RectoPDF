//! **What a release's signature is,** said once for everything that checks one (rule 4): the updater, holding a download to it with `codesign`, and the Find key, holding this running copy to it before it asks the Keychain.

/// The Developer ID rule: an Apple-issued certificate, of this team. In the code-signing requirement language.
#[must_use]
pub fn developer_id_requirement(team: &str) -> String {
    format!(
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\""
    )
}
