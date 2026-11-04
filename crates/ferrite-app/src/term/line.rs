//! The terminal without its pages (the `vt100` feature off, decision 342): a plain command line. Enter starts it
//! (as TERM-2); each line typed runs as after `:` (TERM-12, a leading `:` is taken too), its answer on the next
//! line; lists, dump and load stream as on the console (TERM-13); Ctrl-C leaves. Bluetooth's PIN prompt
//! (LINK-30) comes first, as with the pages.

use super::*;

impl Term {
    /// A key on the command line.
    pub(super) fn line_key(&mut self, k: K) {
        match k {
            K::Ch(b':') if self.clen == 0 => {} // the pages' command key: typed out of habit
            K::Esc => self.clen = 0,
            K::CtrlL => {}
            _ => self.cmd_key(k),
        }
        self.cmd = false;
    }

    /// The banner once, then the prompt and what is typed, when either changed.
    pub(super) fn prompt(&mut self, out: &mut impl Sink) {
        if self.clear {
            if out.room() < 160 {
                return;
            }
            out.put(b"\x1b[0m\r\nFerrite ");
            out.put(version());
            out.put(" terminal: type a command and Enter (help lists them); Ctrl-C leaves.\r\n".as_bytes());
            self.clear = false;
        }
        if self.dirty == 0 || out.room() < ROW_MAX {
            return;
        }
        self.dirty = 0;
        out.put(b"\r\x1b[K");
        if self.pin_gate() {
            // LINK-30: the PIN, alone.
            if !self.note.is_empty() {
                out.put(self.note.as_bytes());
                out.put(b"  ");
            }
            out.put(b"PIN: ");
            for _ in 0..self.pin_n {
                out.put(b"*");
            }
            return;
        }
        let n = self.clen as usize;
        let keep = (self.cols as usize).saturating_sub(4);
        out.put(b"> ");
        out.put(&self.cbuf[n.saturating_sub(keep)..n]);
    }
}
