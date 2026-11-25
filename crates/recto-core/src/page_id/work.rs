//! **The decode-work meter:** what a stream's stages hand on, charged against [`super::Caps::decode_work`] as it's read, stage by stage.
//! A stage that eats its input (ASCIIHex over whitespace, a Flate that is itself fed a Flate) gives little or nothing out, so a count of only the
//! chain's end let a kilobyte stored cost a gigabyte uncharged. Each stage's output, the stored bytes of a stream with no chain too, goes through
//! a [`Counting`] reader sharing one [`Meter`]; when the work left runs out the meter says so and the read fails, and the caller, finding it said so,
//! gives a cap reached, never a failed decode: the budget refuses only, and nothing it counts is hashed or in [`super::PageRead::bytes`].

use std::cell::Cell;
use std::io::{self, Read};
use std::rc::Rc;

/// The decode work a stream's read may still do, shared by every stage of its chain.
#[derive(Clone)]
pub(super) struct Meter {
    left: Rc<Cell<u64>>,
    out: Rc<Cell<bool>>,
}

impl Meter {
    pub(super) fn new(left: u64) -> Self {
        Self {
            left: Rc::new(Cell::new(left)),
            out: Rc::new(Cell::new(false)),
        }
    }

    /// What's left.
    pub(super) fn left(&self) -> u64 {
        self.left.get()
    }

    /// Whether the work ran out: a read that then failed was refused, not undecodable.
    pub(super) fn ran_out(&self) -> bool {
        self.out.get()
    }

    /// `reader`, its bytes charged to this meter.
    pub(super) fn count<'a>(&self, reader: Box<dyn Read + 'a>) -> Box<dyn Read + 'a> {
        Box::new(Counting {
            inner: reader,
            meter: self.clone(),
        })
    }
}

struct Counting<'a> {
    inner: Box<dyn Read + 'a>,
    meter: Meter,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        match self.meter.left.get().checked_sub(read as u64) {
            Some(left) => {
                self.meter.left.set(left);
                Ok(read)
            }
            None => {
                self.meter.left.set(0);
                self.meter.out.set(true);
                Err(io::Error::other("the decode work ran out"))
            }
        }
    }
}
