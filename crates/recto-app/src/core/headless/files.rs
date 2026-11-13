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

/// A file whose contents have a chapter with two sections under it, then a second chapter: something for a chevron to fold. Four pages or more.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer; never over a user's file.
pub fn with_sections(pages: usize) -> Vec<u8> {
    use lopdf::{Object, ObjectId, dictionary};
    let plain: Vec<recto_testpdf::Page> = (0..pages.max(4))
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Page {}", page + 1)])))
        .collect();
    let mut document = lopdf::Document::load_mem(&recto_testpdf::pages(&plain)).unwrap();
    let page: Vec<ObjectId> = document.get_pages().values().copied().collect();
    let [outlines, one, two, first, second] = [(); 5].map(|()| document.new_object_id());
    let dest = |at: usize| Object::Array(vec![page[at].into(), "Fit".into()]);
    let entries = [
        (
            first,
            dictionary! { "Title" => Object::string_literal("1.1 First"), "Parent" => one, "Next" => second, "Dest" => dest(1) },
        ),
        (
            second,
            dictionary! { "Title" => Object::string_literal("1.2 Second"), "Parent" => one, "Prev" => first, "Dest" => dest(2) },
        ),
        (
            one,
            dictionary! { "Title" => Object::string_literal("1 One"), "Parent" => outlines, "Next" => two, "First" => first, "Last" => second, "Count" => 2, "Dest" => dest(0) },
        ),
        (
            two,
            dictionary! { "Title" => Object::string_literal("2 Two"), "Parent" => outlines, "Prev" => one, "Dest" => dest(3) },
        ),
        (outlines, dictionary! { "Type" => "Outlines", "First" => one, "Last" => two, "Count" => 4 }),
    ];
    for (id, entry) in entries {
        document.objects.insert(id, Object::Dictionary(entry));
    }
    let root = document.trailer.get(b"Root").unwrap().as_reference().unwrap();
    document.get_object_mut(root).unwrap().as_dict_mut().unwrap().set("Outlines", outlines);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}
