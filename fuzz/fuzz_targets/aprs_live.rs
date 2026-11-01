//! The radio's receive path for a decoded frame: AprsLive::receive (the AX.25 parse, digipeating, the heard list,
//! messages and acks, our own echoes) then a tick (acks, digipeats and beacons queued), as the APRS task calls them.
//! Input: frames separated by 0xC0 (as KISS would delimit them); the digipeater and our callsign are on, so every
//! branch can run.
#![no_main]
use ferrite_app::aprs::{AprsCfg, DIGI_ON};
use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let live = AprsLive::new();
    let mut cfg = AprsCfg::DEFAULT;
    cfg.digi = DIGI_ON | data.first().copied().unwrap_or(0) & 2;
    let me = b"VE7ABC-9";
    let mut now = 1000u32;
    let mut out = [0u8; FRAME_MAX];
    for f in data.split(|&b| b == 0xC0) {
        let rx = live.receive(f, now, me, &cfg);
        let _ = (rx.row, rx.inbound, rx.inbound_id, rx.state);
        let _ = live.tick(now, Sending::All, true, me, None, &cfg, &mut out);
        let _ = ferrite_app::aprs::wx(f);
        for i in 0..live.stations().min(4) {
            let _ = live.station(i);
        }
        now += 7;
    }
});
