//! The way back to stock over the cable (BACKUP-7, LINK-8, decision 261): once the link session has accepted
//! `STOCK_RESTORE`'s `START`, the UI task comes here and never goes back. The radio is quiet, the store's queue is
//! drained and nothing of Ferrite's is saved again; the cable's frames go to [`ferrite_store::restore::serve`],
//! which writes only inside the driver's own writable ranges (never calibration or the update area), until
//! `FINISH` resets into the bootloader for the stock `.BTF`. Turning the radio off is the only other way out.

use crate::chan::{TERM_IN, TERM_OUT};
use crate::store::{self, Store};
use embassy_time::Timer;
use ferrite_core::link::{self as wire, msg, Status, MAX_PAYLOAD, MAX_WIRE};
use ferrite_core::VERSION;
use ferrite_store::restore::{serve, start, After};
use ferrite_ui::boot::{self, BootInfo};
use ferrite_ui::panel::Panel;

fn screen(lcd: &mut impl Panel, note: &[u8]) {
    let b = BootInfo { version: VERSION, callsign: None, line: Some(b"Back to stock: keep the cable in"), owner: None, checks: &[], note: Some(note) };
    boot::draw(lcd, &b);
}

/// One answer out on the cable, whole (the ring has room for it: answers are short and the PC waits for each).
async fn send(ty: u8, seq: u8, ans: &[u8]) {
    let mut w = [0u8; MAX_WIRE];
    let Some(k) = wire::build(ty | wire::ANSWER, seq, ans, &mut w) else { return };
    while (TERM_OUT.room() as usize) < k {
        Timer::after_millis(1).await;
    }
    for &c in &w[..k] {
        TERM_OUT.put(c);
    }
}

/// Serves the restore from the accepted `START` (its seq) on; never returns.
#[allow(clippy::await_holding_refcell_ref)] // see the idle wait: no other borrower is left
pub async fn run(store: &'static Store, lcd: &mut impl Panel, seq: u8) -> ! {
    // Whatever Ferrite had queued is written first (the restore overwrites it anyway), so the chip is free.
    store::wake(store);
    // Until the Storage task is idle: from here on nothing queues another job (only the UI task does, and it
    // never leaves this function), so the chip borrowed below across awaits is borrowed by no one else.
    while !store.idle() {
        Timer::after_millis(10).await;
    }
    screen(lcd, b"Restoring stock data");
    let (chip, ranges) = (store.nor().size(), store::WRITE_OK);
    let mut ans = [0u8; MAX_PAYLOAD];
    let n = start(&mut *store.nor(), chip, ranges, &mut ans).await;
    send(msg::STOCK_RESTORE, seq, &ans[..n]).await;

    let mut rx = [0u8; MAX_WIRE];
    let mut rn = 0usize;
    let mut overflow = false;
    loop {
        let Some(b) = TERM_IN.get() else {
            Timer::after_millis(1).await;
            continue;
        };
        if b != 0 {
            if rn < rx.len() {
                rx[rn] = b;
                rn += 1;
            } else {
                overflow = true;
            }
            continue;
        }
        let len = core::mem::take(&mut rn);
        if len == 0 || core::mem::take(&mut overflow) {
            continue; // a sync 0x00, or a frame too long for any request
        }
        let (ty, seq, n, after) = match wire::parse(&mut rx[..len]) {
            Err(e) => {
                ans[0] = Status::BadFrame as u8;
                (e.ty, e.seq, 1, After::Next)
            }
            // Only the restore's own requests from here on.
            Ok(f) if f.ty != msg::STOCK_RESTORE => {
                ans[0] = Status::Busy as u8;
                (f.ty, f.seq, 1, After::Next)
            }
            Ok(f) => {
                let (ty, seq) = (f.ty, f.seq);
                let mut p = [0u8; MAX_PAYLOAD];
                let pl = f.payload.len();
                p[..pl].copy_from_slice(f.payload);
                let (n, after) = serve(&mut *store.nor(), chip, ranges, &p[..pl], &mut ans).await;
                (ty, seq, n, after)
            }
        };
        send(ty, seq, &ans[..n]).await;
        if after == After::Reset {
            screen(lcd, b"Done: restarting");
            while TERM_OUT.count() > 0 {
                Timer::after_millis(1).await;
            }
            // The answer's last bytes leave the UART, and the PC has its probes going before the bootloader's
            // ~200 ms window opens: a missed window would start Ferrite over the restored data.
            Timer::after_millis(300).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
    }
}
