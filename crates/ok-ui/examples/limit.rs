//! Draws the word limit as DESIGN.md's Flow section has it, for looking at:
//! the orchard essay with a limit it runs past, at rest and in a run.
//!
//! ```text
//! cargo run -p ok-ui --example limit --offline -- OUT_DIR
//! ```
//!
//! The frames are written to `OUT_DIR` as `w2-flow-limit.png`,
//! `w2-flow-limit-run.png` and `w2-flow-limit-under.png`. A limit belongs
//! to the session, and the session the frames of `shot` are made with has
//! none, so this stands one in front of it: the same desk, and a limit.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ok_host::Host;
use ok_text::{Change, Doc};
use ok_ui::contents::Entry;
use ok_ui::harness::Rig;
use ok_ui::line::Closed;
use ok_ui::session::{Links, Memory, Opened, Session};
use ok_ui::Options;

/// The stand-in session of the frames, with a word limit on its text.
struct Limited {
    desk: Memory,
    limit: Option<usize>,
}

impl Session for Limited {
    fn first(&mut self, host: &mut dyn Host) -> Opened {
        self.desk.first(host)
    }

    fn changed(&mut self, doc: &Doc, change: &Change, host: &mut dyn Host) {
        self.desk.changed(doc, change, host);
    }

    fn name(&self) -> &str {
        self.desk.name()
    }

    fn contents(&self) -> &[Entry] {
        self.desk.contents()
    }

    fn links(&self) -> Option<&Links> {
        self.desk.links()
    }

    fn exists(&self, target: &str) -> bool {
        self.desk.exists(target)
    }

    fn limit(&self) -> Option<usize> {
        self.limit
    }

    fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
    }
}

const THIRD: &str = "He let me think that for about a week. Then he walked me to the one tree he had left alone on purpose, TK the year he planted it, and showed me what keeping all three looks like. The branches had split under the weight. The fruit was the size of walnuts and tasted of nothing.";

fn essay(limit: usize) -> Rig {
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let options = Options { file: Some(desk.join("College/The orchard.md")), folder: Some(desk), look: None, store: false };
    let session = Limited { desk: Memory::new(&options), limit: Some(limit) };
    let mut rig = Rig::with_seams(options, Box::new(session), Box::new(Closed), ok_ui::flow::start());
    rig.size(1920, 1080);
    rig
}

fn draw(out: &Path) -> std::io::Result<()> {
    // Past the limit: 186 words against 150. The rule is set above the row
    // the 151st word is on; from that word on the page is at half strength
    // and the map faint, and the facts say how far over.
    let mut rig = essay(150);
    rig.pace = 90;
    rig.press("enter");
    rig.type_text(THIRD);
    rig.shot(&out.join("w2-flow-limit-run.png"))?;
    rig.wait(12_000);
    rig.shot(&out.join("w2-flow-limit.png"))?;
    // Within it: no rule, and the facts say how far there is to go.
    let mut rig = essay(650);
    rig.press("enter");
    rig.type_text(THIRD);
    rig.wait(12_000);
    rig.shot(&out.join("w2-flow-limit-under.png"))
}

fn main() -> ExitCode {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: limit OUT_DIR");
        return ExitCode::from(2);
    };
    match draw(Path::new(&out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{out}: {why}");
            ExitCode::FAILURE
        }
    }
}
