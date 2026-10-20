# ok

A native Windows writing tool. One window, one .exe, a character grid drawn in
Sun Gallant Demi. Named for the prompt a Sun console prints while it waits for
you to type.

## Rules that are easy to break by accident

- **No third-party crates, anywhere.** Win32 is declared by hand in the shell
  (`#[link(name = "user32")] extern "system"`). `kind = "raw-dylib"` does not
  work here (no dlltool on PATH). There is no windres or rc.exe, so no resource
  script and no embedded manifest: DPI awareness, the icon and the dark title
  bar are all set at runtime.
- **`lto` is on for release builds and works in this folder.** It failed once
  in a scratch folder under `AppData\Roaming`, where Windows Defender held
  rustc's temporary archives open (os error 32). If that shows up, build again
  or set `CARGO_TARGET_DIR` elsewhere; do not just turn lto off.
- **`panic` stays `unwind`.** The shell catches a panic, writes `crash.txt`
  and gives the program one last chance to save.
- **Never put a window on Jasper's screen from a test.** Tests drive the real
  window procedure through a cloaked window (see `ok-win`'s self-test).
- **Always build with `--offline`.** There is nothing to download.
- His writing must never be lost. Anything that touches the save path, the
  journal or recovery gets a test that kills the process mid-write.
- The writing folder stays clean: plain `.md` and `.txt` files any other
  editor can open. ok's own files (journal, history, state) live elsewhere.

## Layout

| Crate | What it holds | May depend on |
| --- | --- | --- |
| `crates/ok-gfx` | Framebuffer, bitmap faces, glyph drawing, PNG writer | nothing |
| `crates/ok-text` | Document, undo, prose motions, wrapping, markup, links, fuzzy match | nothing |
| `crates/ok-host` | The contract between the window shell and the interface | ok-gfx |
| `crates/ok-win` | The Win32 shell: one window, hand-declared FFI | ok-gfx, ok-host |

The design every screen is built from is `DESIGN.md`. Read it before
touching the interface.

Everything on screen is drawn into a `Vec<u32>` (`0x00RRGGBB`, which is also a
Win32 DIB's byte order) on a cell grid. Gallant is 12 by 22 pixels, so a
1920x1080 window is 160 columns by 49 rows.

## Commands

```bash
cargo test --offline                 # every crate
cargo run -p ok-gfx --example specimen --release --offline -- shots/specimen.png
```

`shots/` is where rendered frames go. It is ignored by git.

## Faces

`assets/fonts/gallant/gallant.hex` is from the FreeBSD source tree
(`share/vt/fonts/gallant.hex`). Each built-in face sits in its own folder with
its licence beside it.
