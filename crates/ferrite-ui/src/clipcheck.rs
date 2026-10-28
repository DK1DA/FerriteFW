//! Host only (the `clipcheck` feature): text the drawing cut off without being asked to. A field, a list row, a
//! value or a toast narrower than its text drops the end silently; a text running past its region or off the
//! screen loses its edge. A deliberate cut goes through [`crate::widgets::fit`] first, so what is drawn already
//! fits and nothing is noted here. The screens' coverage tests and the simulator's end-to-end tests turn the
//! check on and fail listing every cut ([`take`]).
//!
//! The radio's image never has this module: the hooks in the drawing code are `cfg`'d out with the feature.

use core::cell::{Cell, RefCell};
use core::fmt;
use core::panic::Location;
use std::string::String;
use std::vec::Vec;

/// One cut: the text as given, the room it had and the width it needed, in pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    /// What happened: "cut" (a field or widget's room), "lines" (a wrapped text with lines left over), "region"
    /// (past its banded region's edge), "screen" (past the screen's edge).
    pub kind: &'static str,
    pub text: String,
    pub room: i32,
    pub width: i32,
    /// Where the widget was called from, when known.
    pub at: Option<&'static Location<'static>>,
    /// What the test harness said it was drawing (the screen's name).
    pub context: String,
}

impl fmt::Display for Clip {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {:?}: {} px in {} px", self.kind, self.text, self.width, self.room)?;
        if let Some(at) = self.at {
            write!(f, " at {}:{}", at.file(), at.line())?;
        }
        if !self.context.is_empty() {
            write!(f, " ({})", self.context)?;
        }
        Ok(())
    }
}

std::thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static SITE: Cell<Option<&'static Location<'static>>> = const { Cell::new(None) };
    static CONTEXT: RefCell<String> = const { RefCell::new(String::new()) };
    static CLIPS: RefCell<Vec<Clip>> = const { RefCell::new(Vec::new()) };
}

/// Start noting cuts on this thread (each test thread has its own), from none.
pub fn start() {
    ON.with(|o| o.set(true));
    CLIPS.with(|c| c.borrow_mut().clear());
}

/// Stop noting cuts on this thread.
pub fn stop() {
    ON.with(|o| o.set(false));
}

/// What is being drawn, named in each cut noted from now on (a screen's name).
pub fn context(s: &str) {
    CONTEXT.with(|c| {
        let mut c = c.borrow_mut();
        c.clear();
        c.push_str(s);
    });
}

/// The cuts noted since [`start`] or the last take, each once.
pub fn take() -> Vec<Clip> {
    CLIPS.with(|c| core::mem::take(&mut *c.borrow_mut()))
}

/// The cuts as lines, for a test's failure message.
pub fn report(clips: &[Clip]) -> String {
    let mut s = String::new();
    for c in clips {
        s.push_str(&std::format!("  {c}\n"));
    }
    s
}

/// A widget's call site, kept while it draws (its inner fields and texts are noted under it), unless an outer
/// widget's site is kept already.
pub struct Site(bool);

impl Site {
    #[track_caller]
    pub fn new() -> Self {
        let at = Location::caller();
        Site(SITE.with(|s| s.get().is_none() && ON.with(|o| o.get()) && {
            s.set(Some(at));
            true
        }))
    }
}

impl Default for Site {
    #[track_caller]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        if self.0 {
            SITE.with(|s| s.set(None));
        }
    }
}

/// A region whose drawing is meant to be cut at its edges ([`crate::canvas::banded_part`]: part of the screen
/// drawn again), kept while it draws: texts crossing a region's edge aren't noted under it.
pub struct PartCut;

impl PartCut {
    pub fn new() -> Self {
        PART.with(|p| p.set(p.get() + 1));
        PartCut
    }
}

impl Default for PartCut {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PartCut {
    fn drop(&mut self) {
        PART.with(|p| p.set(p.get() - 1));
    }
}

std::thread_local! {
    static PART: Cell<u32> = const { Cell::new(0) };
}

/// Note a cut of `text`, `width` px wide, in `room` px.
pub fn note(kind: &'static str, text: &[u8], room: i32, width: i32) {
    if !ON.with(|o| o.get()) || (kind == "region" && PART.with(|p| p.get()) > 0) {
        return;
    }
    let clip = Clip {
        kind,
        text: String::from_utf8_lossy(text).into_owned(),
        room,
        width,
        at: SITE.with(|s| s.get()),
        context: CONTEXT.with(|c| c.borrow().clone()),
    };
    CLIPS.with(|c| {
        let mut c = c.borrow_mut();
        if !c.contains(&clip) {
            c.push(clip);
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::assets::UI_12;
    use crate::fb::FrameBuffer;
    use crate::gfx::Align;
    use crate::widgets as w;

    /// A field too narrow for its text is noted, with the caller; the same text fitted first is not.
    #[test]
    fn notes_unasked_cuts_only() {
        super::start();
        let mut fb = FrameBuffer::new();
        let s = b"A long line of text";
        w::field(&mut fb, 8, 40, 0, 17, 13, Align::Left, s, &UI_12, 0xFFFF, 0);
        let clips = super::take();
        assert_eq!(clips.len(), 1, "{clips:?}");
        assert_eq!((clips[0].kind, clips[0].text.as_str(), clips[0].room), ("cut", "A long line of text", 40));
        assert!(clips[0].at.is_some_and(|l| l.file().ends_with("clipcheck.rs")));
        w::field(&mut fb, 8, 40, 0, 17, 13, Align::Left, w::fit(s, &UI_12, 40), &UI_12, 0xFFFF, 0);
        assert!(super::take().is_empty());
        super::stop();
    }
}
