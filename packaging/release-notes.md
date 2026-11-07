ok: one window, one sheet of Gallant, and a machine that says ok when your words are on disk. Now for Windows, macOS and Linux.

Your writing goes in `ok` in your Documents folder, saved as you type.

## Windows

Download `ok-windows-x64.exe` and run it. There is nothing to install. Windows may say it protected your PC, because the program is not signed: choose **More info**, then **Run anyway**.

## macOS

Download `ok-macos.zip`, open it, and drag `ok` into Applications. It runs on Apple silicon and Intel Macs, macOS 11 or later.

The app is not notarized by Apple, so the first time macOS will refuse to open it. Open it once, then go to **System Settings**, then **Privacy & Security**, and choose **Open Anyway**. Or, in Terminal:

```
xattr -dr com.apple.quarantine /Applications/ok.app
```

On a Mac, Cmd does what Ctrl does on Windows, Option with an arrow moves by word, and Cmd with an arrow goes to the end of the line or the text.

## Linux

For any distribution, on a PC (x86-64) or ARM (a Raspberry Pi, say):

- Ubuntu, Debian, Mint: `ok-linux-x86_64.deb` or `ok-linux-aarch64.deb`, then `sudo apt install ./ok-linux-x86_64.deb`.
- Fedora, openSUSE: `ok-linux-x86_64.rpm` or `ok-linux-aarch64.rpm`, then `sudo dnf install ./ok-linux-x86_64.rpm`.
- Anything else: the `.AppImage`, made runnable with `chmod +x`.
- Or one command, for you alone: `curl -fsSL https://jasperpi.github.io/ok/install.sh | sh`

It needs an X11 or Wayland desktop.

## Your phone

In ok press Esc, type `phone`, and scan a code with the phone's camera. The left code goes through the relay and has a microphone; the right one goes straight to the computer over the Wi-Fi. Android (Chrome) and iPhone (Safari) both work: add the page to the home screen and it opens like an app.

The install wizard at https://jasperpi.github.io/ok/ walks through all of this.

## New in 0.4: structure on the page

Plain words on the line arrange the page, and the line says what Enter will do before you press it:

- `box this` puts a frame round the paragraph (`box this called Packing` names it); `unbox` takes it away and keeps the words.
- `small`, `bigger`, `smaller`, `huge`: small type, double and triple size, a step at a time.
- `side by side` sets this paragraph beside the next; `grid 3` sets the parts from here three to a row.
- `board` makes a surface where notes go anywhere: `move right 10`, `move to 12 4`, or drag them.
- `move up`, `move down`, `width 30`, `wider`, `call it Food`.

With the mouse, take a frame by its left edge to move it and by its right edge to resize it. Everything is one step for Ctrl Z, and it is all saved as ordinary Markdown fences (`::: box Packing` ... `:::`) that other editors read.

The phone's dictation no longer types a sentence twice on Android, and its keys are down to Speak, Return, Scratch and More.

## What is in it

- Type, and it is kept. `ok` is printed once everything is flushed to disk.
- Esc opens the line: part of a title switches texts, `#` goes to a heading, `/` finds, `/old/new` changes.
- `set` lists every option: looks (manila, sun, acme, register, amber, vt100, 5151, wordperfect), faces (Gallant, Spleen, Terminus), colours, measure, platen, limit.
- `[[` links to another text. `keys` lists every key.

On Windows, fixed-pitch faces installed on the system can be chosen too, and full screen covers the whole monitor. On macOS full screen is the Mac's own; on Linux ok is an ordinary window.

`SHA256SUMS.txt` lists the checksum of each download.
