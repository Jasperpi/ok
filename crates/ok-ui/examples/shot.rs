//! Runs a script against the interface with no window, for making still
//! frames and for looking at what a sequence of keys does.
//!
//! ```text
//! cargo run -p ok-ui --example shot --offline -- script.txt [more.txt ...]
//! ```
//!
//! A script is one command to a line. Blank lines and lines starting with
//! `#` are passed over. Paths are relative to the directory the command is
//! run from.
//!
//! Before anything else, a script may say what the program is started with:
//!
//! ```text
//! file PATH             the text to open
//! folder PATH           the writing folder, listed in the contents
//! look NAME             the look to start in: manila, sun, acme, register,
//!                       register night, amber, vt100, 5151, wordperfect
//! ```
//!
//! Then, in any order and as often as wanted:
//!
//! ```text
//! size W H              the client area in pixels (1920 1080 is 160 by 49)
//! look NAME             change the look
//! scale N               1, 2 or 3
//! measure N             cells of text to a row, 40 to 100
//! platen on|off         hold the typing row at the platen, or not
//! scroll smooth|jump    the paper rolls in four frames, or jumps
//! motion on|off         off stands for Windows with animations turned off
//! time Y-M-D H:M        set the wall clock; a writing day begins at 04:00
//! pace MS               time that passes after each typed character
//!
//! type TEXT             type the rest of the line. \n is Enter, \t is Tab,
//!                       \s a space (for one at the end of a line), \\ a
//!                       backslash, \u{e9} a character by its number
//! key CHORD [TIMES]     press a key, with its modifiers joined by +:
//!                       left right up down home end pageup pagedown enter
//!                       tab backspace delete esc f1 to f12, or one
//!                       character for a chord such as ctrl+z or ctrl+=
//! hold MODS             ctrl, alt, shift or several joined by +, held
//!                       until release
//! release               let go of whatever is held
//! click X Y             press and let go of the left button at a pixel
//! click cell COL ROW    the same, in the middle of a cell of the grid
//! press X Y             the button goes down (press, move, lift make a
//! move X Y              drag; each also takes `cell COL ROW`)
//! lift X Y              the button comes up
//! wheel ROWS            the wheel; positive is away from the user
//! wait MS               let time pass, delivering every tick the program
//!                       asks for on the way
//! tick                  one tick, now
//! focus on|off          the window gains or loses the keyboard
//! cursor LINE BYTE      put the cursor in the text, line and byte both
//!                       counted from 0, and bring its row to the platen
//! select L B L B        select from one such place to another
//! clipboard TEXT        put text on the clipboard
//! say TEXT              put a sentence on the say row
//! open PATH             a second launch hands this one a file
//! close                 the window is closed
//!
//! shot PATH.png         draw the screen and write it as a PNG
//! print                 draw the screen and print it as text, one line
//!                       for each row of the grid
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let scripts: Vec<String> = std::env::args().skip(1).collect();
    if scripts.is_empty() {
        eprintln!("usage: shot script.txt [more.txt ...]");
        return ExitCode::from(2);
    }
    for path in scripts {
        let script = match std::fs::read_to_string(&path) {
            Ok(script) => script,
            Err(why) => {
                eprintln!("{path}: {why}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(why) = ok_ui::harness::run(&script) {
            eprintln!("{path}: {why}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
