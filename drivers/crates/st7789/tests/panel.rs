//! The driver against a recording bus.

use st7789::{Bus8080, St7789, Step, CASET, DISPON, RAMWR, RASET, STARTUP, VSCRDEF, VSCSAD};
use uradio_bus::mock::MockDelay;

#[derive(Debug, PartialEq, Clone)]
enum Ev {
    Cmd(u8),
    Data(Vec<u8>),
    Reset(bool),
}

#[derive(Default)]
struct Rec(Vec<Ev>);

impl Bus8080 for Rec {
    fn command(&mut self, c: u8) {
        self.0.push(Ev::Cmd(c));
    }
    fn data(&mut self, d: &[u8]) {
        // Merge consecutive data so tests compare whole payloads.
        if let Some(Ev::Data(v)) = self.0.last_mut() {
            v.extend_from_slice(d);
        } else {
            self.0.push(Ev::Data(d.to_vec()));
        }
    }
    fn reset(&mut self, a: bool) {
        self.0.push(Ev::Reset(a));
    }
}

#[test]
fn startup_matches_stock_order_and_waits() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    let mut d = MockDelay::default();
    p.init(&mut d, 0x0000);
    let ev = p.release().0;
    // Reset pulse, then SWRESET, SLPOUT, COLMOD 0x55, MADCTL 0xC0, INVON, NORON.
    assert_eq!(&ev[..3], &[Ev::Reset(false), Ev::Reset(true), Ev::Reset(false)]);
    assert_eq!(ev[3..11], [Ev::Cmd(0x01), Ev::Cmd(0x11), Ev::Cmd(0x3A), Ev::Data(vec![0x55]), Ev::Cmd(0x36), Ev::Data(vec![0xC0]), Ev::Cmd(0x21), Ev::Cmd(0x13)]);
    // Waits: 1 + 10 + 120 + 150 + 120 ms.
    assert_eq!(d.total_us, 401_000);
    // Cleared (a full-screen window, 240 x 320 x 2 bytes) before the display goes on.
    assert_eq!(ev[ev.len() - 1], Ev::Cmd(DISPON));
    assert_eq!(ev[ev.len() - 2], Ev::Data(vec![0; 240 * 320 * 2]));
    let waits: u32 = STARTUP.iter().map(|s| if let Step::Wait(ms) = s { *ms } else { 0 }).sum();
    assert_eq!(waits, 401);
}

#[test]
fn windows_clip_and_count() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    assert_eq!(p.window(230, 310, 20, 20), 100); // clipped to 10 x 10
    assert_eq!(p.window(240, 0, 1, 1), 0); // off the panel: nothing sent
    assert_eq!(p.window(0, 0, 0, 5), 0);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(CASET), Ev::Data(vec![0, 230, 0, 239]), Ev::Cmd(RASET), Ev::Data(vec![1, 54, 1, 63]), Ev::Cmd(RAMWR)]);
}

#[test]
fn pixels_and_fill_are_big_endian() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.blit(0, 0, 2, 1, &[0x1234, 0xABCD]);
    p.fill(5, 5, 3, 1, 0xF800);
    let ev = p.release().0;
    assert_eq!(ev[5], Ev::Data(vec![0x12, 0x34, 0xAB, 0xCD]));
    assert_eq!(ev[11], Ev::Data(vec![0xF8, 0, 0xF8, 0, 0xF8, 0]));
}

#[test]
fn scrolling() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.scroll_area(20, 20); // status and hint bars fixed, 280 lines scroll
    p.scroll_to(100);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(VSCRDEF), Ev::Data(vec![0, 20, 1, 24, 0, 20]), Ev::Cmd(VSCSAD), Ev::Data(vec![0, 100])]);
}
