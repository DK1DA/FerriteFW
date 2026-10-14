//! The programming cable (UART4): when it may send.
//!
//! - **Keyed (TX-43, TX audit F9):** nothing is sent, so a bias resistor from pin 11 never carries data into the
//!   transmitted audio. The link task asks before every byte; the Radio task marks the radio keyed just before the
//!   key-up plan runs, so a byte in flight is the most that can overlap.
//! - **Speaker mic (SND-14, decision 146):** with one in the side connector nothing is sent at all and PC10 (UART4
//!   TX, the connector's pin 11) is held high as a GPIO output, as stock holds it outside programming mode: Radtel
//!   speaker mics take their mic bias from that pin, so serial data on it (or a UART idling in AF) leaves the mic
//!   without a steady bias.

/// Whether the cable may send a byte now. `dry`: the `tx-dry` build (PA drive held at 0), where the bench reads
/// the chips over the cable while keyed (`research/tx-vs-stock.md` section 6), so keying holds nothing; `spkmic`: a
/// speaker mic is connected (`ferrite_core::acc::ON`): never, in any build.
pub fn may_send(keyed: bool, dry: bool, spkmic: bool) -> bool {
    !spkmic && (dry || !keyed)
}
