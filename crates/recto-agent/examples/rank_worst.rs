//! A corpus for measuring `--rank` at its worst, with no PDFium: `rank_worst <pdf> <corpus-root> <pages>` builds one for the file, with every page holding all of the words `w0` to `w15`, so each page is held in memory for scoring. Then `RECTO_STATE_DIR=... recto local <pdf> search "w0 w1 ... w15" --rank` under `/usr/bin/time -l` gives the whole process's peak. It needs the PDF to have that many pages.

use recto_index::corpus::{self, BuildLimits, CorpusKey, DocMeta, FedPage, FeedError, PageFeed};
use recto_proto::CharRecord;
use std::path::Path;

struct EveryWord(u32);

impl PageFeed for EveryWord {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(self.0)
    }
    fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
        let text: String = (0..16).map(|at| format!("w{at} ")).collect();
        Ok(FedPage {
            chars: text.chars().map(|c| CharRecord::new(c as u32, 0)).collect(),
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }
    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [pdf, root, pages] = args.as_slice() else {
        eprintln!("rank_worst <pdf> <corpus-root> <pages>");
        std::process::exit(2);
    };
    let key = CorpusKey::of_file(Path::new(pdf)).unwrap();
    let outcome = corpus::build(
        Path::new(root),
        key,
        &mut EveryWord(pages.parse().unwrap()),
        &DocMeta::default(),
        &BuildLimits::default(),
    );
    println!("{outcome:?}");
}
