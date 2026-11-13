//! Test files the headless tests need beyond `recto-testpdf`'s.

/// The one chapter a protected test file's contents have, so a test can tell whether anything decrypted got out.
#[allow(dead_code)]
pub const LOCKED_CHAPTER: &str = "Secret chapter";

/// A real protected file, as `recto-cli`'s tests make it: it opens only with the user password given, each page says "Locked page N", and its contents have one
/// chapter, [`LOCKED_CHAPTER`].
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file.
pub fn locked_with(password: &str, pages: usize) -> Vec<u8> {
    locked_allowing(password, pages, lopdf::encryption::Permissions::all())
}

/// The same, with what its permissions allow.
#[allow(clippy::disallowed_methods)] // As above.
pub fn locked_allowing(password: &str, pages: usize, permissions: lopdf::encryption::Permissions) -> Vec<u8> {
    use lopdf::encryption::{EncryptionState, EncryptionVersion};
    use lopdf::{Object, dictionary};
    let plain: Vec<recto_testpdf::Page> = (0..pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Locked page {}", page + 1)])))
        .collect();
    let mut document = lopdf::Document::load_mem(&recto_testpdf::pages(&plain)).unwrap();
    let first_page = *document.get_pages().values().next().unwrap();
    let outlines = document.new_object_id();
    let chapter = document
        .add_object(dictionary! { "Title" => Object::string_literal(LOCKED_CHAPTER), "Parent" => outlines, "Dest" => vec![first_page.into(), "Fit".into()] });
    document.objects.insert(
        outlines,
        Object::Dictionary(dictionary! { "Type" => "Outlines", "First" => chapter, "Last" => chapter, "Count" => 1 }),
    );
    let root = document.trailer.get(b"Root").unwrap().as_reference().unwrap();
    document.get_object_mut(root).unwrap().as_dict_mut().unwrap().set("Outlines", outlines);
    document.trailer.set(
        "ID",
        Object::Array(vec![Object::string_literal("0123456789abcdef"), Object::string_literal("0123456789abcdef")]),
    );
    let encryption = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: password,
        key_length: 128,
        permissions,
    })
    .unwrap();
    document.encrypt(&encryption).unwrap();
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}
