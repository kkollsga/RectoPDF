//! What the worst pages cost line-order repair, for `docs/ENGINEERING.md`. Run on a release build, once per shape, under `/usr/bin/time -l` to read the whole process's peak footprint:
//!
//! `line_order_hostile baseline|one_line|crowd|many_moves`
//!
//! `baseline` builds a page of 2,000,000 characters with boxes and doesn't repair it, which is what the others are compared with.

use recto_index::order::repair_line_order;
use recto_proto::{CharBox, CharRecord};

fn main() {
    let shape = std::env::args().nth(1).unwrap_or_default();
    let count = recto_proto::MAX_PAGE_CHARS as usize;
    let record = CharRecord::new('x' as u32, 0);
    let line_break = CharRecord::new('\n' as u32, CharRecord::GENERATED);
    let half = count / 2;
    let per_line = count / recto_index::order::MAX_LINES + 1;
    let at = |at: usize| -> (CharRecord, CharBox) {
        match shape.as_str() {
            "one_line" if at == half => (line_break, [0.0; 4]),
            "one_line" if at < half => (record, [at as f32 * 40.0, 700.0, at as f32 * 40.0 + 4.0, 707.0]),
            "one_line" => ((record), [(at - half) as f32 * 40.0 + 20.0, 700.0, (at - half) as f32 * 40.0 + 24.0, 707.0]),
            "crowd" if at % per_line == per_line - 1 => (line_break, [0.0; 4]),
            "crowd" => {
                let x = (at % per_line) as f32 * 40.0 + (at / per_line % 3) as f32 * 13.0;
                (record, [x, 700.0, x + 4.0, 707.0])
            }
            "many_moves" => {
                let (row, column) = (at / 200, at % 200);
                let y = (row % 4096) as f32 * 20.0;
                let x = column as f32 * 12.0;
                if column == 199 {
                    (line_break, [0.0; 4])
                } else if row < 4096 {
                    (record, [x, y, x + 4.0, y + 7.0])
                } else if column < 30 {
                    (record, [x + 6.0, y, x + 8.0, y + 7.0])
                } else {
                    (CharRecord::new(' ' as u32, 0), [0.0; 4])
                }
            }
            _ => (record, [at as f32, 700.0, at as f32 + 0.5, 707.0]),
        }
    };
    let (chars, boxes): (Vec<_>, Vec<_>) = (0..count).map(at).unzip();
    if shape == "baseline" {
        println!("baseline: {} characters, not repaired", chars.len());
        return;
    }
    let started = std::time::Instant::now();
    let order = repair_line_order(&chars, &boxes);
    println!("{shape}: {:?}, {} runs moved", started.elapsed(), order.map_or(0, |order| order.runs_moved));
}
