//! The decoder slot. The radio runs one decoder at a time, so the active one lives in a single static of this
//! enum and every decoder shares its memory: the slot is as large as the largest. FT8/FT4 (uft48) and WSPR
//! (uwspr), about 12.75 KB each, join when their audio path is in (milestone 6); the budget below already holds
//! them.

use udsc::Receiver as Dsc;
use uafsk::Demodulator as Afsk;
use upocsag::Receiver as Pocsag;

/// Audio rate the DSP task feeds the decoders.
pub const RATE_HZ: u32 = 12_000;

/// RAM the slot may take: FT8's or WSPR's footprint.
pub const BUDGET: usize = 13 * 1024;

pub enum Decoder {
    Off,
    /// APRS and other 1200-baud AFSK packet (uaprs parses the frames).
    Afsk(Afsk),
    /// Pagers at 512, 1200 and 2400 baud.
    Pocsag(Pocsag),
    /// Marine DSC, channel 70.
    Dsc(Dsc),
}

/// What a decoder found, handed to the event bus.
pub enum Found<'a> {
    Frame(&'a [u8]),
    Page(&'a upocsag::Message<'a>),
    Call(&'a udsc::Call<'a>),
}

impl Decoder {
    pub fn afsk() -> Self {
        Decoder::Afsk(Afsk::new(uafsk::fsk::Config::new(RATE_HZ)))
    }

    pub fn pocsag() -> Self {
        Decoder::Pocsag(Pocsag::new(RATE_HZ))
    }

    pub fn dsc() -> Self {
        Decoder::Dsc(Dsc::new(uafsk::fsk::Config::new(RATE_HZ)))
    }

    /// One block of audio.
    pub fn push(&mut self, block: &[i16], mut found: impl FnMut(Found)) {
        match self {
            Decoder::Off => {}
            Decoder::Afsk(d) => block.iter().for_each(|&x| d.push(x, |f| found(Found::Frame(f)))),
            Decoder::Pocsag(d) => block.iter().for_each(|&x| d.push(x, |m| found(Found::Page(m)))),
            Decoder::Dsc(d) => block.iter().for_each(|&x| d.push(x, |c| found(Found::Call(c)))),
        }
    }
}

const _: () = assert!(core::mem::size_of::<Decoder>() <= BUDGET);

/// A pager message's characters kept: the event log's text field holds 64.
pub const PAGE_TEXT: usize = 64;
/// The largest capcode: POCSAG addresses are 21 bits.
pub const CAPCODE_MAX: u32 = (1 << 21) - 1;

/// A decoded page, small enough to cross from the DSP task to the UI task (DEC-4, DEC-7): 72 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// The 21-bit address (capcode).
    pub capcode: u32,
    pub baud: u16,
    /// Function bits, 0-3.
    pub function: u8,
    /// Read as numeric characters (else 7-bit text).
    pub numeric: bool,
    pub len: u8,
    pub text: [u8; PAGE_TEXT],
}

impl Page {
    /// A message's text, read the way it most likely is: function 0 as numeric digits (the usual convention),
    /// others as 7-bit characters when nine in ten of them are printable (control characters and the padding
    /// senders end with dropped), else as digits after all (systems differ: upocsag's `Message`). A tone-only
    /// page has no text.
    pub fn from(m: &upocsag::Message) -> Page {
        let mut p = Page { capcode: m.address, baud: m.baud, function: m.function, numeric: false, len: 0, text: [0; PAGE_TEXT] };
        let (mut n, mut ok) = (0usize, 0usize);
        for c in m.alpha() {
            n += 1;
            ok += (0x20..0x7F).contains(&c) as usize;
        }
        let mut put = |c: u8| {
            if (p.len as usize) < PAGE_TEXT {
                p.text[p.len as usize] = c;
                p.len += 1;
            }
        };
        if m.function != 0 && n > 0 && ok * 10 >= n * 9 {
            m.alpha().filter(|c| (0x20..0x7F).contains(c)).for_each(&mut put);
        } else {
            p.numeric = true;
            m.numeric().for_each(&mut put);
        }
        while p.len > 0 && p.text[p.len as usize - 1] == b' ' {
            p.len -= 1; // numeric pages end padded with spaces
        }
        p
    }

    pub fn text(&self) -> &[u8] {
        &self.text[..self.len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page through the codec and back, as both kinds.
    #[test]
    fn page_text() {
        extern crate std;
        use std::vec::Vec;
        for (function, content, want, numeric) in [
            (3, upocsag::encode::Content::Alpha(b"STN 4 RESPOND HWY 417"), &b"STN 4 RESPOND HWY 417"[..], false),
            (0, upocsag::encode::Content::Numeric(b"5551234"), &b"5551234"[..], true),
        ] {
            let mut words = Vec::new();
            upocsag::encode::encode(1_234_567, function, content, |w| words.push(w));
            let mut d: upocsag::decode::Decoder = upocsag::decode::Decoder::new();
            d.baud = 1200;
            let mut got = None;
            for w in words {
                if w == upocsag::SYNC {
                    d.sync(); // a batch starts
                } else {
                    d.word(w, |m| got = Some(Page::from(m)));
                }
            }
            d.end(|m| got = got.or(Some(Page::from(m))));
            let p = got.expect("decoded");
            assert_eq!((p.capcode, p.function, p.baud, p.numeric), (1_234_567, function, 1200, numeric));
            assert_eq!(p.text(), want);
        }
    }

    #[test]
    fn slot_size() {
        let n = core::mem::size_of::<Decoder>();
        std::println!("decoder slot: {n} B");
        assert!(n <= BUDGET);
        let mut d = Decoder::pocsag();
        let mut hits = 0;
        d.push(&[0; 1200], |_| hits += 1);
        assert_eq!(hits, 0, "silence decodes nothing");
    }
}
