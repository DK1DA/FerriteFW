//! The BK4829's TX audio set-ups for a data key-up (APRS-14, decision 135; TERM-33): what an APRS burst changes on
//! the keyed chip in every build, and what a bench build's `txtest` arms (`crate::txtest`, decision 151).

/// The BK4829's TX audio set-up for a data key-up (`ferrite_radio::chip::tx_test_filters`): an APRS burst's `data`
/// (APRS-14, decision 135) in every build, and a bench build's `txtest` arming (TERM-33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filters {
    /// The chip's voice settings, as an APRS burst goes out.
    Voice,
    /// REG_2B[2:0] = 111 (TX HPF, LPF and pre-emphasis off), MIC AGC and ALC off. Measured: the injection falls
    /// 6 dB/octave above ~150 Hz.
    Flat,
    /// REG_2B[2:0] = 100 (TX HPF off; pre-emphasis and LPF on: the pre-emphasis cancels the injection's slope), MIC
    /// AGC and ALC off. Measured: +-1 dB from 250 Hz to 2.5 kHz; the data setting.
    Data,
}

impl Filters {
    pub const ALL: [Filters; 3] = [Filters::Voice, Filters::Flat, Filters::Data];

    pub fn name(self) -> &'static str {
        match self {
            Filters::Voice => "voice",
            Filters::Flat => "flat",
            Filters::Data => "data",
        }
    }

    pub fn parse(w: &[u8]) -> Option<Filters> {
        Filters::ALL.into_iter().find(|f| f.name().as_bytes().eq_ignore_ascii_case(w))
    }
}

/// The TX audio set-up an APRS burst (a data key-up) changes and the unkey puts back: always `data` (APRS-14,
/// decision 135: the level then follows the DAC linearly, where the voice set-up's MIC AGC holds it near 4 kHz).
/// A bench build's `txtest` key-up has its own (`crate::txtest::key_up_filters`).
pub fn aprs_filters(aprs: bool) -> Option<Filters> {
    aprs.then_some(Filters::Data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// APRS-14 (decision 135): an APRS burst keys with the `data` TX audio set-up in every build, a voice key-up
    /// with the chip's voice settings (none changed); `txtest` adds its own only in a bench build (decision 151).
    #[test]
    fn aprs_bursts_use_the_data_setup() {
        assert_eq!(aprs_filters(true), Some(Filters::Data));
        assert_eq!(aprs_filters(false), None);
        assert_eq!(Filters::parse(b"DATA"), Some(Filters::Data));
    }
}
