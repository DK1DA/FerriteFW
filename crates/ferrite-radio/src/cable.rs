//! The programming cable (UART4) while keyed (TX-43, TX audit F9): nothing is sent, so a bias resistor from pin 11
//! never carries data into the transmitted audio. The link task asks before every byte; the Radio task marks the
//! radio keyed just before the key-up plan runs, so a byte in flight is the most that can overlap.

/// Whether the cable may send a byte now. `dry`: the `tx-dry` build (PA drive held at 0), where the bench reads
/// the chips over the cable while keyed (`research/tx-vs-stock.md` section 6), so nothing is held.
pub fn may_send(keyed: bool, dry: bool) -> bool {
    dry || !keyed
}
