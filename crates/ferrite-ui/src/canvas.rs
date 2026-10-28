//! Drawing off-screen in bands (DISP-25): a region's ordinary drawing code (blank, fill, text, frame, in any order)
//! runs against a small RAM band a few rows tall, and each band goes to the panel once. Every pixel of the region
//! is written once per redraw, so the panel never shows a blank between two layers. On the radio's bus that
//! blank flickers (found on the black radio, Oct 12, 2026).
//!
//! The band is on the stack: [`BAND_PX`] pixels (3.75 KB), in a function never inlined ([`band_out`],
//! [`band_over`]), so it is there only while a band is drawn: inlined, every drawing function that calls these kept
//! its own copy for its whole run, and a redraw nests several (render, the screen, a card, a widget: ~30 KB of
//! stack measured on the radio, SBAR-23). The drawing runs once per band, so a taller region costs drawing time,
//! not bus time.
use crate::font4::{Font4, Mask4};
use crate::gfx::Gfx;
use crate::panel::Panel;

/// Pixels in one band: 240 x 8.
pub const BAND_PX: usize = 1920;

/// One band: what the drawing code sees as the panel. Pixels outside the band are dropped; on the host's pass for
/// text hints (`fwd`), windows and hints go to the real panel and pixels nowhere.
pub struct Band<'a> {
    buf: &'a mut [u16],
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    win: (i32, i32, i32, i32),
    cur: (i32, i32),
    fwd: Option<&'a mut dyn Panel>,
    /// A canvas drawn inside another's band: everything goes to `fwd`, the outer band.
    thru: bool,
    /// [`banded_over`]: a bit per band pixel that was drawn; only those go to the panel.
    mask: Option<&'a mut [u8]>,
}

impl Band<'_> {
    /// The run of `n` pixels from the cursor along its row: the part inside the band as (buffer index, count), and
    /// the cursor moved on.
    fn run(&mut self, n: u32) -> (Option<(usize, usize, usize)>, u32) {
        let (x, y) = self.cur;
        let (x0, _, x1, _) = self.win;
        let take = ((x1 - x + 1).max(1) as u32).min(n);
        let mut inside = None;
        if y >= self.y && y < self.y + self.h {
            let a = x.max(self.x);
            let b = (x + take as i32).min(self.x + self.w);
            if b > a {
                inside = Some((((y - self.y) * self.w + (a - self.x)) as usize, (a - x) as usize, (b - a) as usize));
            }
        }
        self.cur = if x + take as i32 > x1 { (x0, y + 1) } else { (x + take as i32, y) };
        (inside, take)
    }
}

impl Band<'_> {
    fn mark(&mut self, i: usize, n: usize) {
        if let Some(m) = &mut self.mask {
            for k in i..i + n {
                m[k / 8] |= 1 << (k % 8);
            }
        }
    }
}

impl Panel for Band<'_> {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.win = (x0, y0, x1, y1);
        self.cur = (x0, y0);
        if let Some(p) = &mut self.fwd {
            p.window(x0, y0, x1, y1);
        }
    }

    fn push(&mut self, color: u16, mut count: u32) {
        if let Some(p) = &mut self.fwd {
            if self.thru {
                p.push(color, count);
            }
            return;
        }
        while count > 0 {
            let (inside, take) = self.run(count);
            if let Some((i, _, n)) = inside {
                self.buf[i..i + n].fill(color);
                self.mark(i, n);
            }
            count -= take;
        }
    }

    fn push_buf(&mut self, px: &[u16]) {
        if let Some(p) = &mut self.fwd {
            if self.thru {
                p.push_buf(px);
            }
            return;
        }
        let mut px = px;
        while !px.is_empty() {
            let (inside, take) = self.run(px.len() as u32);
            if let Some((i, off, n)) = inside {
                self.buf[i..i + n].copy_from_slice(&px[off..off + n]);
                self.mark(i, n);
            }
            px = &px[take as usize..];
        }
    }

    fn text_hint(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, s: &[u8]) {
        // the host's pass for text hints knows its region: a text crossing its left or right edge lost that part (its
        // line box may pass the top or bottom: the font's room above and below the ink)
        #[cfg(feature = "clipcheck")]
        if !self.thru && self.fwd.is_some() && self.w > 0 && (x0 < self.x || x1 >= self.x + self.w) {
            crate::clipcheck::note("region", s, self.w, x1 - x0 + 1);
        }
        if let Some(p) = &mut self.fwd {
            p.text_hint(x0, y0, x1, y1, s);
        }
    }

    fn text_style(&mut self, f: &'static crate::font4::Font4, fg: u16, bg: u16) {
        if let Some(p) = &mut self.fwd {
            p.text_style(f, fg, bg);
        }
    }

    fn mask_hint(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u16) {
        if let Some(p) = &mut self.fwd {
            p.mask_hint(x0, y0, x1, y1, c);
        }
    }

    fn text_only(&self) -> bool {
        match &self.fwd {
            Some(p) if self.thru => p.text_only(),
            _ => false,
        }
    }

    /// The band's rows, so text outside them isn't drawn at all (every band runs the whole drawing); all of them
    /// on the pass for text hints.
    fn rows(&self) -> (i32, i32) {
        match &self.fwd {
            Some(p) if self.thru => p.rows(),
            Some(_) => (0, crate::panel::H),
            None => (self.y, self.y + self.h),
        }
    }

    /// Off the screen, except a [`direct`] band over the panel itself: drawing through it is drawing on the panel.
    fn offscreen(&self) -> bool {
        match &self.fwd {
            Some(p) if self.thru => p.offscreen(),
            _ => true,
        }
    }

    fn input_pending(&self) -> bool {
        match &self.fwd {
            Some(p) if self.thru => p.input_pending(),
            _ => false,
        }
    }

    /// A canvas recording its region (DISP-29): what is drawn through this band goes into the recording.
    fn rec(&mut self) -> Option<&mut dyn Record> {
        match &mut self.fwd {
            Some(p) if self.thru => p.rec(),
            _ => None,
        }
    }
}

/// Inside another canvas's band: draw straight into it (a second band on the stack would cost 3.75 KB more).
fn thru(lcd: &mut dyn Panel) -> Band<'_> {
    Band { buf: &mut [], x: 0, y: 0, w: 0, h: 0, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: true, mask: None }
}

/// A band that passes everything straight to `lcd`, the panel or another band, and is what `lcd` is (on screen
/// or off): drawing code that runs both on the panel and inside a canvas takes a `&mut Band` and is compiled once,
/// not once per panel type (home: 14.8 KB, size review Oct 20, 2026). Each call goes through `lcd`'s
/// vtable, once per primitive (a fill, a text row, a band), never per pixel.
pub fn direct(lcd: &mut dyn Panel) -> Band<'_> {
    thru(lcd)
}

/// `SINGLE_PASS`, DISP-29 on (the default): regions record their drawing once, and a new screen paints each pixel once. Off (the
/// bench's `lcd paint old`, TERM-40): as before Oct 23, 2026, the drawing runs once per band and a new screen is
/// cleared first, to compare the two by eye on the radio.
#[cfg(not(feature = "std"))]
static SINGLE_PASS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
// On the host each test thread has its own, so a test comparing the two can't change another's drawing.
#[cfg(feature = "std")]
std::thread_local! {
    static SINGLE_PASS: core::cell::Cell<bool> = const { core::cell::Cell::new(true) };
}

/// [`SINGLE_PASS`]: on (the default) or off (`lcd paint old`).
pub fn single_pass() -> bool {
    #[cfg(feature = "std")]
    return SINGLE_PASS.with(|c| c.get());
    #[cfg(not(feature = "std"))]
    SINGLE_PASS.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn set_single_pass(on: bool) {
    #[cfg(feature = "std")]
    SINGLE_PASS.with(|c| c.set(on));
    #[cfg(not(feature = "std"))]
    SINGLE_PASS.store(on, core::sync::atomic::Ordering::Relaxed)
}

/// Host only: how many times a region's drawing code has run, recorded or per band, on this thread (DISP-29's
/// tests count the slow-flash work by it).
#[cfg(feature = "std")]
pub fn draw_runs() -> u32 {
    DRAW_RUNS.with(|c| c.get())
}
#[cfg(feature = "std")]
std::thread_local! {
    static DRAW_RUNS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}
#[inline(always)]
fn ran() {
    #[cfg(feature = "std")]
    DRAW_RUNS.with(|c| c.set(c.get() + 1));
}

/// The most of a band buffer a region's recording may take, in pixels (DISP-29): half, so a large home card (about
/// 1 KB of recording) still leaves six rows a band.
const REC_MAX_PX: usize = BAND_PX / 2;

/// A region's drawing, recorded once and replayed (DISP-29). The drawing code above the primitives (a card's texts,
/// hashes and layout, in the slow flash on the radio) then runs once per region, not once per band: a home card is
/// 9 to 16 bands, and running it per band was most of a SLOT redraw's 140 ms (Oct 23, 2026). Only the primitives
/// are recorded ([`crate::gfx::Gfx`]'s fill, rounded fill and frame, text and masks, and nested [`banded`]
/// regions), with their arguments as given, so a replay draws exactly what the drawing would have: the same calls
/// in the same order. A drawing that does not fit, or that writes pixels itself, is drawn as before.
pub struct Rec<'a> {
    buf: &'a mut [u8],
    n: usize,
    /// Something did not fit, or was not a primitive: draw per band instead.
    over: bool,
    /// The panel's [`Panel::input_pending`] as the recording began (a new screen's long drawing stops for input).
    pending: bool,
}

/// Where the primitives go while a region records ([`Panel::rec`]).
pub trait Record {
    /// Append bytes; too many and the recording is given up.
    fn put(&mut self, b: &[u8]);
    /// Give the recording up.
    fn fail(&mut self);
}

const OP_FILL: u8 = 0;
const OP_FILL_R: u8 = 1;
const OP_FRAME_R: u8 = 2;
const OP_TEXT: u8 = 3;
const OP_MASK: u8 = 4;
/// A nested [`banded`] region (x, y, w, h, bg), its drawing up to the matching [`OP_END`].
const OP_BAND: u8 = 5;
const OP_END: u8 = 6;
const PTR: usize = core::mem::size_of::<usize>();

impl Record for Rec<'_> {
    fn put(&mut self, b: &[u8]) {
        if self.over || self.n + b.len() > self.buf.len() {
            self.over = true;
            return;
        }
        self.buf[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
    }
    fn fail(&mut self) {
        self.over = true;
    }
}

impl dyn Record + '_ {
    /// Coordinates as 16 bits: anything wider is not recorded.
    fn put_i(&mut self, v: &[i32]) {
        for &v in v {
            match i16::try_from(v) {
                Ok(v) => self.put(&v.to_le_bytes()),
                Err(_) => self.fail(),
            }
        }
    }

    pub(crate) fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u16) {
        self.put(&[OP_FILL]);
        self.put_i(&[x, y, w, h]);
        self.put(&c.to_le_bytes());
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rounded(&mut self, frame: bool, x: i32, y: i32, w: i32, h: i32, r: i32, c: u16) {
        self.put(&[if frame { OP_FRAME_R } else { OP_FILL_R }]);
        self.put_i(&[x, y, w, h, r]);
        self.put(&c.to_le_bytes());
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn text(&mut self, x: i32, baseline: i32, s: &[u8], f: &'static Font4, fg: u16, bg: u16, y0: i32, y1: i32) {
        let Ok(len) = u8::try_from(s.len()) else {
            return self.fail();
        };
        self.put(&[OP_TEXT]);
        self.put_i(&[x, baseline, y0.clamp(-32768, 32767), y1.clamp(-32768, 32767)]);
        self.put(&fg.to_le_bytes());
        self.put(&bg.to_le_bytes());
        self.put(&(f as *const Font4 as usize).to_ne_bytes());
        self.put(&[len]);
        self.put(s);
    }

    pub(crate) fn mask(&mut self, x: i32, y: i32, m1: &'static Mask4, c1: u16, m2: Option<(&'static Mask4, u16)>, bg: u16) {
        self.put(&[OP_MASK]);
        self.put_i(&[x, y]);
        self.put(&(m1 as *const Mask4 as usize).to_ne_bytes());
        self.put(&c1.to_le_bytes());
        let (p2, c2) = m2.map_or((0, 0), |(m, c)| (m as *const Mask4 as usize, c));
        self.put(&p2.to_ne_bytes());
        self.put(&c2.to_le_bytes());
        self.put(&bg.to_le_bytes());
    }

    fn band(&mut self, x: i32, y: i32, w: i32, h: i32, bg: u16) {
        self.put(&[OP_BAND]);
        self.put_i(&[x, y, w, h]);
        self.put(&bg.to_le_bytes());
    }
}

/// Recording: the primitives come here through [`Panel::rec`]; anything that writes pixels itself can't be
/// replayed.
impl Panel for Rec<'_> {
    fn window(&mut self, _: i32, _: i32, _: i32, _: i32) {
        self.over = true;
    }
    fn push(&mut self, _: u16, _: u32) {
        self.over = true;
    }
    fn push_buf(&mut self, _: &[u16]) {
        self.over = true;
    }
    fn offscreen(&self) -> bool {
        true
    }
    fn input_pending(&self) -> bool {
        self.pending
    }
    fn rec(&mut self) -> Option<&mut dyn Record> {
        Some(self)
    }
}

/// One recorded primitive.
enum Op<'a> {
    Fill(i32, i32, i32, i32, u16),
    Rounded(bool, i32, i32, i32, i32, i32, u16),
    Text(i32, i32, &'a [u8], &'static Font4, u16, u16, i32, i32),
    Mask(i32, i32, &'static Mask4, u16, Option<(&'static Mask4, u16)>, u16),
    /// A nested region and its recording.
    Band(i32, i32, i32, i32, u16, &'a [u8]),
}

/// Reads a recording back in order.
struct Rd<'a>(&'a [u8]);
impl<'a> Rd<'a> {
    fn u8(&mut self) -> u8 {
        let (&b, rest) = self.0.split_first().unwrap_or((&0, &[]));
        self.0 = rest;
        b
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }
    fn i(&mut self) -> i32 {
        self.u16() as i16 as i32
    }
    fn ptr(&mut self) -> usize {
        let mut b = [0u8; PTR];
        b.iter_mut().for_each(|v| *v = self.u8());
        usize::from_ne_bytes(b)
    }
    fn bytes(&mut self, n: usize) -> &'a [u8] {
        let (s, rest) = self.0.split_at(n.min(self.0.len()));
        self.0 = rest;
        s
    }

    /// The next primitive (None at the end, or at the end of a nested region).
    fn op(&mut self) -> Option<Op<'a>> {
        if self.0.is_empty() {
            return None;
        }
        Some(match self.u8() {
            OP_FILL => Op::Fill(self.i(), self.i(), self.i(), self.i(), self.u16()),
            op @ (OP_FILL_R | OP_FRAME_R) => Op::Rounded(op == OP_FRAME_R, self.i(), self.i(), self.i(), self.i(), self.i(), self.u16()),
            OP_TEXT => {
                let (x, baseline, y0, y1, fg, bg) = (self.i(), self.i(), self.i(), self.i(), self.u16(), self.u16());
                // SAFETY: recorded from a `&'static Font4` (`text` above).
                let f = unsafe { &*(self.ptr() as *const Font4) };
                let n = self.u8() as usize;
                Op::Text(x, baseline, self.bytes(n), f, fg, bg, y0, y1)
            }
            OP_MASK => {
                let (x, y) = (self.i(), self.i());
                // SAFETY: both recorded from `&'static Mask4`s (`mask` above; 0: none).
                let m1 = unsafe { &*(self.ptr() as *const Mask4) };
                let (c1, p2, c2) = (self.u16(), self.ptr(), self.u16());
                let m2 = (p2 != 0).then(|| (unsafe { &*(p2 as *const Mask4) }, c2));
                Op::Mask(x, y, m1, c1, m2, self.u16())
            }
            OP_BAND => {
                let (x, y, w, h, bg) = (self.i(), self.i(), self.i(), self.i(), self.u16());
                let start = self.0;
                while self.op().is_some() {}
                let len = start.len() - self.0.len();
                // the inner ops, without the OP_END just read
                Op::Band(x, y, w, h, bg, &start[..len.saturating_sub(1)])
            }
            _ => return None,
        })
    }
}

/// Draw a recording ([`Rec`]) into `p`. Into a band (each primitive clips to its rows) a nested region is its
/// background and drawing, as [`banded`] draws inside a canvas; onto the panel (`bands`) it goes in bands.
fn replay(rec: &[u8], p: &mut dyn Panel, bands: bool, at: &mut RleCache) {
    // Each band replays the whole recording: anything outside its rows is skipped here, before any work (a text's
    // width, a fill's theming). Most of a SLOT redraw's 35 ms of replay went on that (Oct 23, 2026).
    let (r0, r1) = p.rows();
    let out = |a: i32, b: i32| b <= r0 || a >= r1;
    let mut r = Rd(rec);
    while let Some(op) = r.op() {
        let skip = match op {
            Op::Fill(_, y, _, h, _) | Op::Rounded(_, _, y, _, h, _, _) | Op::Band(_, y, _, h, _, _) => out(y, y + h),
            Op::Text(_, baseline, _, f, _, _, y0, y1) => {
                let top = baseline - f.ascent as i32;
                out(y0.max(top), y1.min(top + f.line_h as i32))
            }
            Op::Mask(_, y, m1, ..) => out(y, y + m1.h as i32),
        };
        if skip {
            continue;
        }
        match op {
            Op::Fill(x, y, w, h, c) => {
                let t0 = crate::prof::now();
                p.fill(x, y, w, h, c);
                crate::prof::add(crate::prof::FILL, t0);
            }
            Op::Rounded(frame, x, y, w, h, rad, c) => {
                let t0 = crate::prof::now();
                if frame {
                    p.frame_r(x, y, w, h, rad, c);
                } else {
                    p.fill_r(x, y, w, h, rad, c);
                }
                crate::prof::add(crate::prof::FILL, t0);
            }
            Op::Text(x, baseline, s, f, fg, bg, y0, y1) => match at.get(f, s) {
                Some(at) => {
                    p.text4_rows_at(x, baseline, s, f, fg, bg, y0, y1, at);
                }
                None => {
                    p.text4_rows(x, baseline, s, f, fg, bg, y0, y1);
                }
            },
            Op::Mask(x, y, m1, c1, m2, bg) => p.mask4(x, y, m1, c1, m2, bg),
            Op::Band(x, y, w, h, bg, inner) if bands => band_rec(p, x, y, w, h, bg, inner),
            Op::Band(x, y, w, h, bg, inner) => {
                p.fill(x, y, w, h, bg);
                replay(inner, p, false, at);
            }
        }
    }
}

/// Texts in a run-length (big) font keep their place in their glyphs' rows from one band to the next: each band
/// decoded every glyph from its top again, 1001 glyph rows for 255 drawn on a SLOT redraw (Oct 23, 2026). Keyed by
/// where the text is in the recording; the bands go down the region, so the rows only move on.
#[derive(Default)]
struct RleCache {
    keys: [usize; 4],
    at: [crate::gfx::RleAt; 4],
}

impl RleCache {
    fn get(&mut self, f: &Font4, s: &[u8]) -> Option<&mut crate::gfx::RleAt> {
        if !f.rle {
            return None;
        }
        let key = s.as_ptr() as usize;
        let i = match self.keys.iter().position(|&k| k == key) {
            Some(i) => i,
            None => {
                let i = self.keys.iter().position(|&k| k == 0)?; // full: decode from the top, as before
                self.keys[i] = key;
                i
            }
        };
        Some(&mut self.at[i])
    }
}

/// Draw the region (x, y, w, h) with `draw`, band by band, each band sent to `lcd` once. `draw` paints the whole
/// region in absolute coordinates, as it would on the panel (anything it leaves unpainted shows as `bg`). It runs
/// once, recorded, and the recording is replayed into each band (DISP-29); a drawing too big to record runs once
/// per band. On the host (`std`) a last pass hands the text hints to the panel, for the simulator's screen reading.
#[inline]
pub fn banded<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, bg: u16, mut draw: impl FnMut(&mut Band)) {
    region(lcd, x, y, w, h, bg, &mut draw);
}

/// [`banded`] for part of the screen drawn again (a box gone, the screen under it repainted): the drawing covers
/// more than the region, and is meant to be cut at its edges. The same as [`banded`] on the radio; the host's
/// `clipcheck` doesn't count those cuts.
#[inline]
pub fn banded_part<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, bg: u16, draw: impl FnMut(&mut Band)) {
    #[cfg(feature = "clipcheck")]
    let _part = crate::clipcheck::PartCut::new();
    banded(lcd, x, y, w, h, bg, draw);
}

/// [`banded`]'s body, one copy for every caller: generic, it was copied into each call site, and DISP-29's recording
/// branch made those copies 18 KB of slow flash (Oct 23, 2026).
#[inline(never)]
fn region(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, bg: u16, draw: &mut dyn FnMut(&mut Band)) {
    if w <= 0 || h <= 0 {
        return;
    }
    if let Some(r) = lcd.rec() {
        // inside a recording: the region, its drawing, its end (DISP-29)
        r.band(x, y, w, h, bg);
        draw(&mut thru(lcd));
        if let Some(r) = lcd.rec() {
            r.put(&[OP_END]);
        }
        return;
    }
    if lcd.offscreen() {
        Gfx::fill(lcd, x, y, w, h, bg);
        return draw(&mut thru(lcd));
    }
    band_out(lcd, x, y, w, h, bg, draw);
}

/// What a region's bands draw: its recording, or the drawing itself once per band.
enum Src<'a> {
    Rec(&'a [u8]),
    Draw(&'a mut dyn FnMut(&mut Band)),
}

/// [`banded`]'s bands: the buffer, and the recording at its start, live in this frame alone.
#[inline(never)]
fn band_out(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, bg: u16, draw: &mut dyn FnMut(&mut Band)) {
    let mut buf = [0u16; BAND_PX];
    // SAFETY: the first REC_MAX_PX pixels as bytes (u8 has no alignment, and any bytes are valid u16s after).
    let bytes = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), REC_MAX_PX * 2) };
    let mut rec = Rec { buf: bytes, n: 0, over: !single_pass(), pending: false };
    if !rec.over {
        ran();
        let t0 = crate::prof::now();
        draw(&mut thru(&mut rec));
        crate::prof::add(crate::prof::REC, t0);
    }
    let n = if rec.over { 0 } else { rec.n };
    let (head, px) = buf.split_at_mut(n.div_ceil(2));
    if n > 0 {
        // SAFETY: as above, the recording's bytes.
        let rec = unsafe { core::slice::from_raw_parts(head.as_ptr().cast::<u8>(), n) };
        bands(lcd, px, x, y, w, h, bg, Src::Rec(rec));
    } else {
        bands(lcd, px, x, y, w, h, bg, Src::Draw(draw));
    }
}

/// A recorded region onto the panel, in bands of its own.
#[inline(never)]
fn band_rec(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, bg: u16, rec: &[u8]) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut buf = [0u16; BAND_PX];
    bands(lcd, &mut buf, x, y, w, h, bg, Src::Rec(rec));
}

/// The region in bands of `px`, each sent to `lcd` once; on the host a last pass for the text hints.
#[allow(clippy::too_many_arguments)]
fn bands(lcd: &mut dyn Panel, px: &mut [u16], x: i32, y: i32, w: i32, h: i32, bg: u16, mut src: Src) {
    let w = w.min(px.len() as i32);
    let rows = (px.len() as i32 / w).max(1);
    let bg = crate::theme::map_bg(bg);
    let mut by = y;
    let mut at = RleCache::default();
    let tb = crate::prof::now();
    while by < y + h {
        let bh = rows.min(y + h - by);
        let n = (w * bh) as usize;
        let t0 = crate::prof::now();
        px[..n].fill(bg);
        crate::prof::add(crate::prof::PUSH, t0);
        {
            let mut band = Band { buf: &mut px[..n], x, y: by, w, h: bh, win: (0, 0, 0, 0), cur: (0, 0), fwd: None, thru: false, mask: None };
            match &mut src {
                Src::Rec(r) => replay(r, &mut band, false, &mut at),
                Src::Draw(d) => {
                    ran();
                    d(&mut band)
                }
            }
        }
        let t0 = crate::prof::now();
        lcd.window(x, by, x + w - 1, by + bh - 1);
        lcd.push_buf(&px[..n]);
        crate::prof::add(crate::prof::PUSH, t0);
        by += bh;
    }
    crate::prof::add(crate::prof::BANDS, tb);
    if cfg!(feature = "std") {
        // the text hints: windows and hints to the panel, pixels nowhere
        let mut band = Band { buf: &mut [], x, y, w, h, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: false, mask: None };
        match &mut src {
            Src::Rec(r) => replay(r, &mut band, false, &mut RleCache::default()),
            Src::Draw(d) => d(&mut band),
        }
    }
}

/// Rows of the screen [`fresh`] works out its background for at a time.
const COVER_ROWS: i32 = 16;

/// A whole new screen in the region (x, y, w, h) over `bg` (a screen opened, the panel woken): DISP-29. `draw`
/// paints the content as it would over a cleared region, recorded once; then `bg` goes only where no opaque
/// primitive (a fill, a text's line box, a mask, a nested [`banded`] region) paints, and the recording is drawn
/// over it, nested regions in bands. Clearing the region first sent most of its pixels twice: 263-281 KB for a
/// 153 KB screen on every screen open (Oct 23, 2026). Returns false, having drawn nothing, when the drawing doesn't
/// fit the recording (or writes pixels itself): the caller then puts back what the drawing changed and clears
/// and draws as before.
#[inline(never)]
pub fn fresh(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, bg: u16, draw: &mut dyn FnMut(&mut Band)) -> bool {
    if lcd.offscreen() || !single_pass() {
        return false;
    }
    let mut buf = [0u8; BAND_PX * 2];
    let mut rec = Rec { buf: &mut buf, n: 0, over: false, pending: lcd.input_pending() };
    ran();
    let t0 = crate::prof::now();
    draw(&mut thru(&mut rec));
    crate::prof::add(crate::prof::REC, t0);
    let (n, over) = (rec.n, rec.over);
    if over {
        return false;
    }
    let rec = &buf[..n];
    let t0 = crate::prof::now();
    cover(lcd, rec, x, y, w, h, bg);
    crate::prof::add(crate::prof::COVER, t0);
    replay(rec, lcd, true, &mut RleCache::default());
    true
}

/// The background of [`fresh`]: `bg` wherever no opaque primitive of `rec` paints, rows of runs merged into
/// rectangles while they repeat.
#[inline(never)]
fn cover(lcd: &mut dyn Panel, rec: &[u8], x: i32, y: i32, w: i32, h: i32, bg: u16) {
    const WORDS: usize = (crate::panel::W as usize).div_ceil(32);
    let (x0, x1) = (x.max(0), (x + w).min(crate::panel::W));
    let (y0, y1) = (y.max(0), (y + h).min(crate::panel::H));
    // runs not yet sent: (x, w) from row `since`, while every row since has had the same runs
    let mut open: [(i16, i16); 8] = [(0, 0); 8];
    let (mut nopen, mut since) = (0usize, y0);
    let flush = |lcd: &mut dyn Panel, open: &[(i16, i16)], since: i32, to: i32| {
        for &(rx, rw) in open {
            lcd.fill(rx as i32, since, rw as i32, to - since, bg);
        }
    };
    let mut cy = y0;
    while cy < y1 {
        let ch = COVER_ROWS.min(y1 - cy);
        let mut bits = [[0u32; WORDS]; COVER_ROWS as usize];
        let mut mark = |ax: i32, ay: i32, aw: i32, ah: i32| {
            let (a, e) = (ax.max(x0), (ax + aw).min(x1));
            if a >= e {
                return;
            }
            // the columns a..e as whole words: a word at a time, not a bit (Oct 23, 2026: 32-55 ms a screen open)
            let (wa, we) = (a as usize / 32, (e - 1) as usize / 32);
            let (lo, hi) = (!0u32 << (a % 32), !0u32 >> (31 - (e - 1) % 32));
            for row in ay.max(cy)..(ay + ah).min(cy + ch) {
                let b = &mut bits[(row - cy) as usize];
                if wa == we {
                    b[wa] |= lo & hi;
                } else {
                    b[wa] |= lo;
                    b[wa + 1..we].fill(!0);
                    b[we] |= hi;
                }
            }
        };
        let mut r = Rd(rec);
        while let Some(op) = r.op() {
            match op {
                Op::Fill(ax, ay, aw, ah, _) | Op::Band(ax, ay, aw, ah, _, _) => mark(ax, ay, aw, ah),
                Op::Rounded(false, ax, ay, aw, ah, rad, _) => {
                    // the corners show what is under them: only the cross is opaque
                    let rad = rad.min(aw / 2).min(ah / 2).max(0);
                    mark(ax + rad, ay, aw - 2 * rad, ah);
                    mark(ax, ay + rad, aw, ah - 2 * rad);
                }
                Op::Rounded(true, ..) => {}
                Op::Text(tx, baseline, s, f, _, _, ty0, ty1) => {
                    let top = baseline - f.ascent as i32;
                    let (a, b) = (ty0.max(top), ty1.min(top + f.line_h as i32));
                    mark(tx, a, crate::gfx::text4_width(s, f), b - a);
                }
                Op::Mask(mx, my, m1, ..) => mark(mx, my, m1.w as i32, m1.h as i32),
            }
        }
        for (k, b) in bits[..ch as usize].iter().enumerate() {
            let row = cy + k as i32;
            // this row's uncovered runs
            let mut runs: [(i16, i16); 8] = [(0, 0); 8];
            let (mut n, mut c, mut many) = (0usize, x0, false);
            while c < x1 {
                let s = next_bit(b, c, x1, false);
                if s >= x1 {
                    break;
                }
                c = next_bit(b, s, x1, true);
                if n < runs.len() {
                    runs[n] = (s as i16, (c - s) as i16);
                    n += 1;
                } else {
                    many = true;
                    lcd.fill(s, row, c - s, 1, bg);
                }
            }
            if many || runs[..n] != open[..nopen] {
                flush(lcd, &open[..nopen], since, row);
                (open, nopen, since) = (runs, n, row);
            }
        }
        cy += ch;
    }
    flush(lcd, &open[..nopen], since, y1);
}

/// The first column from `c` (before `end`) whose bit is `set`, or `end`: a word at a time.
fn next_bit(b: &[u32], mut c: i32, end: i32, set: bool) -> i32 {
    while c < end {
        let w = b[c as usize / 32];
        let m = if set { w } else { !w } >> (c % 32);
        if m != 0 {
            return (c + m.trailing_zeros() as i32).min(end);
        }
        c = (c / 32 + 1) * 32;
    }
    end
}

/// As [`banded`], for a shape drawn over what is already on screen (a popup with rounded corners): only the
/// pixels `draw` paints go to the panel, a run at a time, so the rest keeps what was under it.
#[inline]
pub fn banded_over<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, mut draw: impl FnMut(&mut Band)) {
    over(lcd, x, y, w, h, &mut draw);
}

/// [`banded_over`]'s body, one copy for every caller (as [`region`]).
#[inline(never)]
fn over(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, draw: &mut dyn FnMut(&mut Band)) {
    if w <= 0 || h <= 0 {
        return;
    }
    if lcd.offscreen() {
        return draw(&mut thru(lcd));
    }
    band_over(lcd, x, y, w, h, draw);
}

/// [`banded_over`]'s bands: the buffer and mask live in this frame alone.
#[inline(never)]
fn band_over(lcd: &mut dyn Panel, x: i32, y: i32, w: i32, h: i32, draw: &mut dyn FnMut(&mut Band)) {
    let w = w.min(BAND_PX as i32);
    let rows = (BAND_PX as i32 / w).max(1);
    let mut buf = [0u16; BAND_PX];
    let mut mask = [0u8; BAND_PX / 8];
    let mut by = y;
    while by < y + h {
        let bh = rows.min(y + h - by);
        let n = (w * bh) as usize;
        mask.fill(0);
        {
            let mut band =
                Band { buf: &mut buf[..n], x, y: by, w, h: bh, win: (0, 0, 0, 0), cur: (0, 0), fwd: None, thru: false, mask: Some(&mut mask) };
            draw(&mut band);
        }
        for r in 0..bh {
            let mut c = 0;
            while c < w {
                let k = (r * w + c) as usize;
                if mask[k / 8] & 1 << (k % 8) == 0 {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < w && mask[((r * w + c) as usize) / 8] & 1 << ((r * w + c) as usize % 8) != 0 {
                    c += 1;
                }
                lcd.window(x + start, by + r, x + c - 1, by + r);
                let a = (r * w + start) as usize;
                lcd.push_buf(&buf[a..a + (c - start) as usize]);
            }
        }
        by += bh;
    }
    if cfg!(feature = "std") {
        let mut band = Band { buf: &mut [], x, y, w, h, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: false, mask: None };
        draw(&mut band);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;
    use crate::gfx::Gfx;

    /// DISP-25: a blank-then-draw region drawn through the bands ends as drawn directly, with every pixel written
    /// once and no flicker.
    #[test]
    fn banded_drawing_is_flicker_free() {
        let draw = |p: &mut dyn FnMut(i32, i32, i32, i32, u16)| {
            p(10, 20, 100, 30, 0x1111);
            p(15, 25, 20, 10, 0x2222);
            p(60, 22, 30, 25, 0x3333);
        };
        let mut direct = FrameBuffer::new();
        draw(&mut |x, y, w, h, c| direct.fill(x, y, w, h, c));
        let mut fb = FrameBuffer::new();
        fb.window(0, 0, 239, 319);
        fb.push(0x7777, 240 * 320);
        fb.begin_pass();
        banded(&mut fb, 10, 20, 100, 30, 0, |b| draw(&mut |x, y, w, h, c| b.fill(x, y, w, h, c)));
        let r = fb.end_pass(true);
        assert_eq!(r.flicker, 0);
        assert_eq!(r.writes, 3000, "each pixel of the region once");
        for y in 20..50 {
            for x in 10..110 {
                assert_eq!(fb.at(x, y), direct.at(x, y), "{x},{y}");
            }
        }
    }

    use crate::assets::{DIG_28, LOGO112_RING, LOGO112_WIND, NAME_22, UI_12};

    /// A screen of noise: what a region is drawn over.
    fn noisy() -> FrameBuffer {
        let mut fb = FrameBuffer::new();
        let px: std::vec::Vec<u16> = (0..240 * 320u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u16).collect();
        fb.window(0, 0, 239, 319);
        fb.push_buf(&px);
        fb
    }

    fn assert_same(a: &FrameBuffer, b: &FrameBuffer, what: &str) {
        for y in 0..320 {
            for x in 0..240 {
                assert_eq!(a.at(x, y), b.at(x, y), "{what}: ({x}, {y})");
            }
        }
    }

    /// `draw` in the region (0, 40, 240, 120) over `bg` through the bands, against clearing it and drawing on the
    /// panel; how many times the drawing ran.
    fn banded_vs_direct(what: &str, draw: &dyn Fn(&mut dyn Panel)) -> u32 {
        let mut want = noisy();
        Gfx::fill(&mut want as &mut dyn Panel, 0, 40, 240, 120, 0x0841);
        draw(&mut want);
        let mut fb = noisy();
        let runs = core::cell::Cell::new(0);
        banded(&mut fb, 0, 40, 240, 120, 0x0841, |b| {
            runs.set(runs.get() + 1);
            draw(b)
        });
        assert_same(&fb, &want, what);
        runs.get()
    }

    /// DISP-29: a region records its drawing once and replays it into each band (two layers of a mask, rounded
    /// fills and frames, more big-font texts than the decode cache keeps); one that can't be recorded (a coordinate
    /// past 16 bits, a text over 255 bytes, pixels written straight) is drawn once per band instead. Both end as
    /// the drawing would on the panel.
    #[test]
    fn recorded_or_not_a_region_draws_the_same() {
        let shapes = |p: &mut dyn Panel| {
            p.mask4(64, 40, &LOGO112_RING, 0xFFFF, Some((&LOGO112_WIND, 0x001F)), 0x0841);
            p.fill_r(10, 60, 50, 40, 8, 0xF800);
            p.frame_r(70, 60, 50, 40, 8, 0x07E0);
            for (i, t) in [&b"12"[..], b"34", b"56", b"78", b"90"].iter().enumerate() {
                p.text4(10 + 45 * i as i32, 150, t, &DIG_28, 0xFFFF, 0x0841);
            }
            p.text4(10, 110, b"Ferrite", &NAME_22, 0xFFE0, 0x0841);
        };
        assert_eq!(banded_vs_direct("recorded", &shapes), 1, "recorded once");
        let long = [b'x'; 300];
        let wide = |p: &mut dyn Panel| {
            shapes(p);
            p.fill(40_000, 50, 10, 10, 0xFFFF);
        };
        let text = |p: &mut dyn Panel| {
            shapes(p);
            p.text4(0, 140, &long, &UI_12, 0xFFFF, 0x0841);
        };
        let pixels = |p: &mut dyn Panel| {
            shapes(p);
            p.window(5, 45, 6, 46);
            p.push(0x1234, 4);
        };
        for (what, d) in [("wide", &wide as &dyn Fn(&mut dyn Panel)), ("long text", &text), ("pixels", &pixels)] {
            assert!(banded_vs_direct(what, d) > 2, "{what}: drawn per band");
        }
    }

    /// DISP-29: a new screen's background goes only where nothing opaque is drawn, even with more gaps in a row than
    /// it keeps runs for, and around a nested region; the result is what clearing first and drawing gives. A
    /// drawing that writes pixels itself is refused, nothing drawn.
    #[test]
    fn a_fresh_screen_matches_clear_then_draw() {
        let draw = |p: &mut dyn Panel| {
            for i in 0..12 {
                p.fill(5 + 19 * i, 30, 9, 20, 0xF800 + i as u16);
            }
            p.frame_r(20, 60, 100, 40, 6, 0x07E0);
            p.fill_r(130, 60, 100, 40, 6, 0x001F);
            p.text4(10, 130, b"Fresh", &UI_12, 0xFFFF, 0x0841);
            p.mask4(64, 150, &LOGO112_RING, 0xFFFF, None, 0x0841);
        };
        let mut want = noisy();
        Gfx::fill(&mut want as &mut dyn Panel, 0, 20, 240, 280, 0x0841);
        draw(&mut want);
        banded(&mut want, 30, 270, 100, 20, 0x2104, |b| b.fill(40, 275, 20, 10, 0xFFFF));
        let mut fb = noisy();
        assert!(fresh(&mut fb, 0, 20, 240, 280, 0x0841, &mut |b: &mut Band| {
            draw(b);
            banded(b, 30, 270, 100, 20, 0x2104, |b| b.fill(40, 275, 20, 10, 0xFFFF));
        }));
        assert_same(&fb, &want, "fresh");
        let before = fb.px.clone();
        assert!(!fresh(&mut fb, 0, 20, 240, 280, 0, &mut |b: &mut Band| {
            b.window(0, 20, 0, 20);
            b.push(1, 1);
        }));
        assert!(fb.px == before, "refused: nothing drawn");
    }

    /// An empty region draws nothing, banded or over.
    #[test]
    fn empty_regions_draw_nothing() {
        let mut fb = FrameBuffer::new();
        banded(&mut fb, 0, 0, 0, 10, 0xFFFF, |b| b.fill(0, 0, 10, 10, 0xFFFF));
        banded(&mut fb, 0, 0, 10, -1, 0xFFFF, |b| b.fill(0, 0, 10, 10, 0xFFFF));
        banded_over(&mut fb, 0, 0, 10, 0, |b| b.fill(0, 0, 10, 10, 0xFFFF));
        assert!(fb.writes.iter().all(|&w| w == 0));
    }
}
