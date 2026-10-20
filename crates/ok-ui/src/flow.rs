//! Flow: what the screen does while he writes.
//!
//! DESIGN.md's "Flow" table lives here: Rest, Writing, Run, Breath, Settle
//! and Taking the wheel, with their triggers and their fades. [`Writing`]
//! is that table, and is what [`start`] hands the program.
//!
//! The interface is the [`Flow`] trait. Every method has a body that does
//! nothing, and [`Rest`] is the flow that takes them all as they are: the
//! screen at rest, always, which is what a test of something else wants.
//! The call sites in `app.rs` and the painters are exercised by
//! `tests/seams.rs` with a stand-in flow, and the table itself by
//! `tests/flow.rs` through the whole interface.
//!
//! # How the table is kept
//!
//! - Rest to Writing: the 8th character typed with no gap over 2 s between
//!   one key and the next. The sides (contents, labels, links, facts) go to
//!   faint over 900 ms.
//! - Writing to Run: 20 words begun since then. Every paragraph but his
//!   own goes to half and the sides to ground, over 1500 ms.
//! - Breath: 4 s without a key in Writing or Run. The paragraphs come back
//!   over 400 ms and nothing else does. The next character typed is Writing
//!   again, the sides left where they were, and another 20 words a Run.
//! - Settle: 10 s without a key. Everything back over 400 ms, and if 100
//!   words or more were begun since the first character of the stretch,
//!   `143 words without stopping.` for 6 s.
//! - Taking the wheel (the cursor leaves its paragraph, Esc, any chord, the
//!   mouse): Rest in 120 ms, from any state.
//! - Enter keeps the paragraph just finished in ink until 8 characters of
//!   the next have been typed, then lets it go to half over 1500 ms.
//!   Backspace, Delete and movement inside the paragraph only keep a pause
//!   from being counted.
//! - With animations turned off in Windows every fade is one step.
//!
//! # What a flow receives
//!
//! - [`Flow::note`] for everything he does, as an [`Act`], with the host
//!   for the time ([`Host::now_ms`]) and for whether Windows has animations
//!   turned off ([`Host::reduced_motion`], when every fade is one step).
//!   The acts arrive in the order they happen and each after it has
//!   happened, so the text already holds a typed letter when the flow hears
//!   of it.
//! - [`Flow::tick`] on every tick of the shell. A pause is noticed here:
//!   the tick is asked for with [`Flow::wake_in_ms`] (the 4 s breath, the
//!   10 s settle), and a tick every frame with [`Flow::animating`] while a
//!   fade is running. A sentence handed back goes on the say row
//!   (`143 words without stopping.`); [`Said`] says for how long.
//!
//! # What a flow answers
//!
//! Cheaply, because the painters ask for every row of every frame:
//! - [`Flow::part`]: the strength of the contents, the labels, the links
//!   and the facts. It is given the strength the thing has at rest (the
//!   open text's name in ink, the rest at half, leader dots faint) and
//!   hands back what it has now. While the line is driving a pane, that
//!   pane is drawn at rest without asking.
//! - [`Flow::text`]: the strength of a line of the text, given its number
//!   and whether it is in the paragraph the cursor is in. Marks are drawn
//!   at half of whatever is answered, or at it when it is weaker than half.
//!
//! What a flow may not touch, because it is never dimmed in any state: the
//! paragraph he is in, the cursor, the map's marks, the notch, the change
//! bars and `ok`. None of them asks anything, except that the lines of his
//! own paragraph come to [`Flow::text`] with `own` set and must go back as
//! they came. A flow never draws, never moves the cursor and never hides
//! the pointer (the interface hides it at the first keystroke and shows it
//! again when the mouse moves); a strength is all that leaves this file.

use ok_host::Host;

use crate::look::Strength;
use crate::machine::thousands;

/// Something he did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    /// A character went into the text.
    Typed(char),
    /// Backspace or Delete took something out.
    Erased,
    /// Enter was pressed on the page; these are the first and last lines
    /// of the paragraph the cursor was in, as they were numbered before.
    Entered { first: usize, last: usize },
    /// The cursor moved without the text changing. `left` is true when it
    /// left the paragraph it was in.
    Moved { left: bool },
    /// Any chord: a key pressed with Ctrl or Alt held. It comes before
    /// whatever the chord then does.
    Chord,
    /// Esc.
    Escape,
    /// The mouse moved, clicked or turned its wheel.
    Mouse,
}

/// The things beside the page whose strength changes with the flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Contents,
    Labels,
    Links,
    /// The name and the counts on the machine line.
    Facts,
}

/// A sentence for the say row and how long it stays.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Said {
    pub text: String,
    /// Milliseconds before it is taken down again. `None` leaves it until
    /// the next key.
    pub for_ms: Option<u64>,
}

/// What the screen does while he writes, as the interface sees it.
pub trait Flow {
    /// He did something.
    fn note(&mut self, _act: Act, _host: &dyn Host) {}

    /// The shell ticked. Hands back a sentence when there is one to say.
    fn tick(&mut self, _host: &dyn Host) -> Option<Said> {
        None
    }

    /// True while a fade is running and a frame is wanted every tick.
    fn animating(&self, _now_ms: u64) -> bool {
        false
    }

    /// How long from `now_ms` the flow wants a tick, if it wants one.
    fn wake_in_ms(&self, _now_ms: u64) -> Option<u64> {
        None
    }

    /// The strength of something beside the page that is `rest` at rest.
    fn part(&self, _part: Part, rest: Strength, _now_ms: u64) -> Strength {
        rest
    }

    /// The strength of one line of the text that is `rest` at rest. `own`
    /// is true for the lines of the paragraph the cursor is in, which must
    /// come back as they went in.
    fn text(&self, _line: usize, _own: bool, rest: Strength, _now_ms: u64) -> Strength {
        rest
    }
}

/// The do-nothing flow: everything at its rest strength, always.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rest;

impl Flow for Rest {}

/// The states of the table that last. Settle and Taking the wheel are not
/// states but ways back to [`State::Rest`], one slow and one quick.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    #[default]
    Rest,
    Writing,
    Run,
    Breath,
}

/// The 8th character typed with no gap over two seconds begins Writing.
const START_CHARS: u32 = 8;
const START_GAP_MS: u64 = 2000;
/// Twenty words typed in Writing begin a Run.
const RUN_WORDS: u32 = 20;
/// Four seconds without a key is a Breath; ten, a Settle.
const BREATH_MS: u64 = 4000;
const SETTLE_MS: u64 = 10_000;
/// How long each fade takes.
const WRITING_FADE_MS: u64 = 900;
const RUN_FADE_MS: u64 = 1500;
const BACK_FADE_MS: u64 = 400;
const WHEEL_FADE_MS: u64 = 120;
/// A Settle after this many words says how many there were, for this long.
const SAID_WORDS: usize = 100;
const SAID_MS: u64 = 6000;
/// Enter leaves the paragraph just finished in ink until this many
/// characters of the next have been typed.
const KEPT_CHARS: u32 = 8;

/// How far the sides have gone: 0 at rest, then at faint, then at ground.
const AT_FAINT: u16 = 255;
const AT_GROUND: u16 = 510;
/// How far the other paragraphs have gone: 0 at rest, then at half.
const AT_HALF: u16 = 255;

/// One thing on its way from one level to another. A level is a place on
/// the road a strength takes, not a strength, so that a fade turned round
/// half way sets off from exactly where it had got to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Fade {
    from: u16,
    to: u16,
    start_ms: u64,
    over_ms: u64,
}

impl Fade {
    /// The level at a moment.
    fn level(&self, now_ms: u64) -> u16 {
        let gone = now_ms.saturating_sub(self.start_ms);
        if self.over_ms == 0 || gone >= self.over_ms {
            return self.to;
        }
        let (from, to) = (self.from as i64, self.to as i64);
        (from + (to - from) * gone as i64 / self.over_ms as i64) as u16
    }

    /// True while it is still on its way.
    fn running(&self, now_ms: u64) -> bool {
        self.from != self.to && now_ms.saturating_sub(self.start_ms) < self.over_ms
    }

    /// Sets off for `to` from wherever it is, to arrive `over_ms` from now.
    /// One already bound for `to` is left to arrive when it was going to.
    fn go(&mut self, to: u16, now_ms: u64, over_ms: u64) {
        if self.to == to {
            return;
        }
        let from = self.level(now_ms);
        *self = Fade { from, to, start_ms: now_ms, over_ms: if from == to { 0 } else { over_ms } };
    }
}

/// The paragraph Enter has just finished, which stays in ink for a while.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Kept {
    first: usize,
    last: usize,
    /// Characters typed since that Enter.
    typed: u32,
    /// How far it has gone toward what the other paragraphs are: nowhere
    /// until it is let go.
    fade: Fade,
}

/// The flow of DESIGN.md: what fades, how far and how fast, as he writes,
/// pauses and takes the wheel.
///
/// Nothing here runs by itself. Thresholds of time are noticed on a tick
/// that was asked for through [`Flow::wake_in_ms`], or on the next thing he
/// does if no tick came; a fade is a start, an end and a duration, read off
/// whenever a painter asks. At rest it asks for nothing at all.
#[derive(Clone, Debug, Default)]
pub struct Writing {
    state: State,
    /// Characters typed one after another with no gap over two seconds,
    /// while at rest.
    streak: u32,
    /// When he last pressed a key of the kind writing is made of: a
    /// character, Enter, Backspace, Delete, a move inside his paragraph.
    last_key_ms: Option<u64>,
    /// Words begun since the first character of this stretch of writing,
    /// and since Writing began or was last resumed after a Breath.
    words: usize,
    since: u32,
    /// The stretch between spaces being typed has been counted as a word.
    counted: bool,
    /// Contents, labels, links and facts.
    sides: Fade,
    /// Every paragraph but his own.
    others: Fade,
    kept: Option<Kept>,
    /// A sentence waiting for the next tick to carry it to the say row.
    to_say: Option<Said>,
    /// Windows has animations turned off: every fade is one step.
    reduced: bool,
}

impl Writing {
    /// Which state of the table the flow is in.
    pub fn state(&self) -> State {
        self.state
    }

    /// How long a fade of `ms` takes on this machine.
    fn over(&self, ms: u64) -> u64 {
        if self.reduced {
            0
        } else {
            ms
        }
    }

    /// A key of the kind writing is made of was pressed: the pause, if one
    /// was being counted, starts again from now.
    fn touch(&mut self, now_ms: u64) {
        if self.last_key_ms.is_some() {
            self.last_key_ms = Some(now_ms);
        }
    }

    /// Notices the thresholds of time that have passed since the last key:
    /// a Breath after four seconds, a Settle after ten.
    fn catch_up(&mut self, now_ms: u64) {
        let Some(last) = self.last_key_ms else { return };
        let idle = now_ms.saturating_sub(last);
        if matches!(self.state, State::Writing | State::Run) && idle >= BREATH_MS {
            // All paragraphs back to ink. Nothing else returns.
            self.state = State::Breath;
            self.others.go(0, now_ms, self.over(BACK_FADE_MS));
            self.kept = None;
        }
        if self.state == State::Breath && idle >= SETTLE_MS {
            if self.words >= SAID_WORDS {
                self.to_say = Some(Said { text: format!("{} words without stopping.", thousands(self.words)), for_ms: Some(SAID_MS) });
            }
            self.rest(now_ms, BACK_FADE_MS);
        }
    }

    /// Back to Rest, everything returning over `ms`.
    fn rest(&mut self, now_ms: u64, ms: u64) {
        self.state = State::Rest;
        self.sides.go(0, now_ms, self.over(ms));
        self.others.go(0, now_ms, self.over(ms));
        self.kept = None;
        self.streak = 0;
        self.words = 0;
        self.since = 0;
        self.counted = false;
        self.last_key_ms = None;
    }

    /// Counts a typed character toward the words of this stretch. A word
    /// is counted at its first letter or digit, as the facts count them.
    fn count(&mut self, ch: char) {
        if ch.is_whitespace() {
            self.counted = false;
        } else if ch.is_alphanumeric() && !self.counted {
            self.counted = true;
            self.words += 1;
            if self.state != State::Rest {
                self.since += 1;
            }
        }
    }

    fn typed(&mut self, ch: char, now_ms: u64) {
        let gap = self.last_key_ms.map(|last| now_ms.saturating_sub(last));
        self.last_key_ms = Some(now_ms);
        match self.state {
            State::Rest => {
                if !gap.is_some_and(|gap| gap <= START_GAP_MS) {
                    // The first character after a pause starts afresh.
                    (self.streak, self.words, self.counted) = (0, 0, false);
                }
                self.streak += 1;
                self.count(ch);
                if self.streak >= START_CHARS {
                    self.state = State::Writing;
                    self.since = 0;
                    self.sides.go(AT_FAINT, now_ms, self.over(WRITING_FADE_MS));
                }
            }
            State::Breath => {
                // He is writing again. The sides stay where they were; the
                // other paragraphs wait for another twenty words.
                self.state = State::Writing;
                self.since = 0;
                self.count(ch);
            }
            State::Writing | State::Run => self.count(ch),
        }
        if self.state == State::Writing && self.since >= RUN_WORDS {
            self.state = State::Run;
            self.others.go(AT_HALF, now_ms, self.over(RUN_FADE_MS));
            self.sides.go(AT_GROUND, now_ms, self.over(RUN_FADE_MS));
        }
        let over = self.over(RUN_FADE_MS);
        if let Some(kept) = &mut self.kept {
            kept.typed += 1;
            if kept.typed == KEPT_CHARS {
                kept.fade.go(AT_HALF, now_ms, over);
            }
        }
    }
}

impl Flow for Writing {
    fn note(&mut self, act: Act, host: &dyn Host) {
        let now_ms = host.now_ms();
        self.reduced = host.reduced_motion();
        self.catch_up(now_ms);
        match act {
            Act::Typed(ch) => self.typed(ch, now_ms),
            // Small corrections are part of writing: they keep the pause
            // from being counted and change nothing else.
            Act::Erased | Act::Moved { left: false } => self.touch(now_ms),
            Act::Entered { first, last } => {
                self.touch(now_ms);
                self.counted = false;
                // The paragraph just finished stays in ink. Enter pressed
                // again before anything is typed has finished nothing new.
                let fresh = !self.kept.is_some_and(|kept| kept.typed == 0);
                if self.state != State::Rest && fresh {
                    self.kept = Some(Kept { first, last, typed: 0, fade: Fade::default() });
                }
            }
            // Taking the wheel.
            Act::Moved { left: true } | Act::Chord | Act::Escape | Act::Mouse => self.rest(now_ms, WHEEL_FADE_MS),
        }
    }

    fn tick(&mut self, host: &dyn Host) -> Option<Said> {
        self.reduced = host.reduced_motion();
        self.catch_up(host.now_ms());
        self.to_say.take()
    }

    fn animating(&self, now_ms: u64) -> bool {
        self.sides.running(now_ms) || self.others.running(now_ms) || self.kept.is_some_and(|kept| kept.fade.running(now_ms))
    }

    fn wake_in_ms(&self, now_ms: u64) -> Option<u64> {
        if self.to_say.is_some() {
            return Some(0);
        }
        let last = self.last_key_ms?;
        let due = match self.state {
            State::Rest => return None,
            State::Writing | State::Run => last.saturating_add(BREATH_MS),
            State::Breath => last.saturating_add(SETTLE_MS),
        };
        Some(due.saturating_sub(now_ms))
    }

    fn part(&self, _part: Part, rest: Strength, now_ms: u64) -> Strength {
        // From what it is at rest to faint, and from there to ground. What
        // is no stronger than faint at rest has only the second leg to go.
        let level = self.sides.level(now_ms);
        let faint = rest.min(Strength::FAINT);
        if level <= AT_FAINT {
            rest.toward(faint, level as u8)
        } else {
            faint.toward(Strength::GROUND, (level - AT_FAINT).min(255) as u8)
        }
    }

    fn text(&self, line: usize, own: bool, rest: Strength, now_ms: u64) -> Strength {
        if own {
            return rest;
        }
        let mut level = self.others.level(now_ms);
        if let Some(kept) = self.kept.filter(|kept| kept.first <= line && line <= kept.last) {
            level = level.min(kept.fade.level(now_ms));
        }
        rest.toward(rest.min(Strength::HALF), level.min(255) as u8)
    }
}

/// The flow the program starts with.
pub fn start() -> Box<dyn Flow> {
    Box::new(Writing::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_is_read_off_at_any_moment_and_turns_round_from_where_it_is() {
        let mut fade = Fade::default();
        assert!(!fade.running(0) && fade.level(0) == 0);
        fade.go(255, 1000, 900);
        assert!(fade.running(1000) && fade.running(1899) && !fade.running(1900));
        assert_eq!((fade.level(1000), fade.level(1450), fade.level(1900), fade.level(99_999)), (0, 127, 255, 255));
        // Asked again for where it is already going, it keeps its time.
        fade.go(255, 1450, 900);
        assert_eq!(fade.level(1900), 255);
        // Turned round half way, it sets off from where it had got to.
        fade.go(0, 1450, 120);
        assert_eq!((fade.level(1450), fade.level(1510), fade.level(1570)), (127, 64, 0));
        // With no time to take it is there at once, and is not running.
        fade.go(255, 2000, 0);
        assert!(fade.level(2000) == 255 && !fade.running(2000));
        // Going nowhere is not a fade.
        fade.go(255, 3000, 900);
        assert!(!fade.running(3000));
    }

    #[test]
    fn a_word_is_counted_at_its_first_letter_or_digit() {
        let mut flow = Writing::default();
        for ch in "It's -- 3 well-known words, (yes) ".chars() {
            flow.count(ch);
        }
        assert_eq!(flow.words, 5);
    }
}
