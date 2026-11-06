//! Optional runs over real files, read-only. Set `RECTO_BENCH100` to the folder to run; without it these pass having done nothing, which
//! they say on stderr. Real files prove the worker survives them. Never pins a number from one machine.

mod common;

use common::{renderer, tile_surface};
use recto_render_service::{Policy, RenderFailure, TileRect, TileRequest, TileResult};
use std::path::PathBuf;

fn corpus() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("RECTO_BENCH100") else {
        eprintln!("RECTO_BENCH100 isn't set, so no corpus files were read.");
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
        .collect();
    files.sort();
    files
}

/// Failure: a real file makes the worker fail, or the host rejects a real reply, which would mean the validators are stricter than PDFium.
#[test]
fn real_files_draw_and_read_without_a_rejection() {
    let surface = tile_surface();
    let (mut documents, mut pages, mut chars) = (0, 0, 0usize);
    for path in corpus() {
        let mut doc = renderer(&path, Policy::default());
        let count = doc.page_count().unwrap_or_else(|failure| panic!("{}: {failure:?}", path.display()));
        documents += 1;
        for page in 0..count.min(3) {
            let (width, height) = doc.page_size(page).unwrap();
            let rect = TileRect { x: 0, y: 0, width: 512, height: 512 };
            let scale = (1024.0 / width.max(height)).min(2.0);
            let result = doc.render_tile(&TileRequest { page, rect, scale, stride: 2048, generation: 0 }, &surface);
            assert!(matches!(result, Ok(TileResult::Done(_))), "{} p{page}: {result:?}", path.display());
            let text = doc.page_characters(page, true, false).unwrap_or_else(|failure| panic!("{} p{page}: {failure:?}", path.display()));
            pages += 1;
            chars += text.chars.len();
        }
        assert_eq!(doc.stats().rejections, 0, "{}", path.display());
    }
    eprintln!("read {documents} files, {pages} pages, {chars} characters");
}

/// Failure: a real password-protected file is reported as damaged, or the right password doesn't open it. Needs `RECTO_ENCRYPTED_PDF`, a file
/// whose password is in `RECTO_ENCRYPTED_PASSWORD`: this repo generates no encrypted fixture.
#[test]
fn a_real_encrypted_file_asks_for_its_password() {
    let (Some(path), Some(password)) = (std::env::var_os("RECTO_ENCRYPTED_PDF"), std::env::var("RECTO_ENCRYPTED_PASSWORD").ok()) else {
        eprintln!("RECTO_ENCRYPTED_PDF isn't set, so no encrypted file was tried.");
        return;
    };
    let path = PathBuf::from(path);
    let mut without = renderer(&path, Policy::default());
    assert_eq!(without.page_count().unwrap_err(), RenderFailure::NeedsPassword);
    let mut with = recto_render_service::DocRenderer::new(&path, Some(password.as_bytes()), common::real_config(), Policy::default()).unwrap();
    assert!(with.page_count().unwrap() > 0);
}
