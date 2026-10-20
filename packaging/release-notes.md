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

Download `ok-linux-x64.tar.gz`, unpack it, and run `./ok`. `INSTALL.txt` inside says how to add it to your applications menu. It needs an X11 or Wayland desktop.

## What is in it

- Type, and it is kept. `ok` is printed once everything is flushed to disk.
- Esc opens the line: part of a title switches texts, `#` goes to a heading, `/` finds, `/old/new` changes.
- `set` lists every option: looks (manila, sun, acme, register, amber, vt100, 5151, wordperfect), faces (Gallant, Spleen, Terminus), colours, measure, platen, limit.
- `[[` links to another text. `keys` lists every key.

On Windows, fixed-pitch faces installed on the system can be chosen too, and full screen covers the whole monitor. On macOS full screen is the Mac's own; on Linux ok is an ordinary window.

`SHA256SUMS.txt` lists the checksum of each download.
