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

#[cfg(test)]
mod tests {
    use super::*;

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
