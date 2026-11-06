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

In ok press Esc, type `phone`, and scan a code with the phone's camera. The left code goes through the relay and has a microphone; the right one goes straight to the computer over the Wi-Fi. Android works now; iPhone is next.

The install wizard at https://jasperpi.github.io/ok/ walks through all of this.

## What is in it

- Type, and it is kept. `ok` is printed once everything is flushed to disk.
- Esc opens the line: part of a title switches texts, `#` goes to a heading, `/` finds, `/old/new` changes.
- `set` lists every option: looks (manila, sun, acme, register, amber, vt100, 5151, wordperfect), faces (Gallant, Spleen, Terminus), colours, measure, platen, limit.
- `[[` links to another text. `keys` lists every key.

On Windows, fixed-pitch faces installed on the system can be chosen too, and full screen covers the whole monitor. On macOS full screen is the Mac's own; on Linux ok is an ordinary window.

`SHA256SUMS.txt` lists the checksum of each download.
