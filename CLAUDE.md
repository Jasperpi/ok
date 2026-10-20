# ok

A writing tool for Windows, macOS and Linux. One window, one program, a
character grid drawn in Sun Gallant Demi. Named for the prompt a Sun console prints while it waits for
you to type.

## Rules that are easy to break by accident

- **No third-party crates, except minifb in `ok-mini`.** That one crate opens
  the window on macOS and Linux, and is a dependency only off Windows. On
  Windows nothing comes from outside: Win32 is declared by hand in the shell
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
- **Build with `--offline` on Windows.** There is nothing to download there.
  macOS and Linux fetch minifb the first time.
- **Platform code stays in three places:** `ok-win` (Windows window),
  `ok-mini` (macOS and Linux window) and `ok-store/src/sys_*.rs` (the file
  calls the save layer needs). Everything else is the same on every system.
  The store's other code speaks Win32 error numbers; `sys_unix.rs` turns
  errno into them.
- **Test macOS and Linux behaviour on a Linux file system.** Under WSL that
  means `~`, not `/mnt/c`: tests on the Windows drive see Windows file
  semantics and fail for reasons a Linux machine never has. Tests that are
  about Windows paths or Windows file sharing are `#[cfg(windows)]`.
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
| `crates/ok-mini` | The macOS and Linux shell: one minifb window | ok-gfx, ok-host, minifb |
| `crates/ok-store` | The writing folder, journal, saving, history, recovery | ok-text |
| `crates/ok-ui` | The interface: sheet, contents, map, the line | ok-gfx, ok-text, ok-host, ok-store |

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

Releases are built by `.github/workflows/release.yml`: push a tag such as
`v0.2.0` and it tests on macOS and Linux, builds all three systems, and puts
`ok-windows-x64.exe`, `ok-macos.zip` (one app for Apple silicon and Intel)
and `ok-linux-x64.tar.gz` on the release, with `packaging/release-notes.md`
as its text.

## Faces

`assets/fonts/gallant/gallant.hex` is from the FreeBSD source tree
(`share/vt/fonts/gallant.hex`). Each built-in face sits in its own folder with
its licence beside it.
