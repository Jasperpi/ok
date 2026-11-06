# ok

A writing program. One window, one sheet of paper set in Sun's old console face, and a machine that prints **ok** when your words are on the disk. Type from your phone too, or talk to it.

**[Start here: the install wizard](https://jasperpi.github.io/ok/)** asks what computer and phone you have and gets you the right download.

## Download

| Computer | Download | Then |
| --- | --- | --- |
| Windows PC | [ok-windows-x64.exe](https://github.com/Jasperpi/ok/releases/latest/download/ok-windows-x64.exe) | Open it. If Windows says it protected your PC: More info, then Run anyway. |
| Mac | [ok-macos.zip](https://github.com/Jasperpi/ok/releases/latest/download/ok-macos.zip) | Drag ok into Applications. The first time: System Settings, then Privacy and Security, then Open Anyway. |
| Ubuntu, Debian, Mint | [.deb for PCs](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-x86_64.deb), [.deb for ARM](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-aarch64.deb) | `sudo apt install ./ok-linux-x86_64.deb` |
| Fedora, openSUSE | [.rpm for PCs](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-x86_64.rpm), [.rpm for ARM](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-aarch64.rpm) | `sudo dnf install ./ok-linux-x86_64.rpm` |
| Any other Linux | [AppImage for PCs](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-x86_64.AppImage), [AppImage for ARM](https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-aarch64.AppImage) | `chmod +x ok-linux-*.AppImage` and open it |

Or, on any Linux, one command installs it for you alone:

```bash
curl -fsSL https://jasperpi.github.io/ok/install.sh | sh
```

## Your phone

Open ok, press **Esc**, type `phone` and press **Enter**. Scan a code with the phone's camera:

- **The code on the left** goes through a relay and works anywhere. Its page has a microphone: tap Speak and talk, and say *period*, *comma*, *new paragraph* or *scratch that*. Your words are sealed with a key only your phone and computer have; the relay cannot read them.
- **The code on the right** goes straight to the computer over your Wi-Fi, and nothing leaves the house.

Each page has a key to switch to the other. To keep ok on the phone like an app: on an iPhone, **Share**, then **Add to Home Screen**; on Android, Chrome's menu, then **Add to home screen**. On an iPhone, speaking uses its own dictation, which must be on in Settings, General, Keyboard.

## Using it

Just type. Everything is saved as you go, into `ok` in your Documents folder as plain `.md` files any other editor can open.

**Esc** goes to the line at the foot of the sheet. There, a few letters of a title open a text, `#` goes to a heading, `/` finds, `set` changes the look, the face and the rest, and `keys` lists every key.

## Building it

```bash
cargo build --release
cargo test
```

Rust, no other libraries on Windows (the window is Win32, declared by hand). On macOS and Linux the window comes from [minifb](https://github.com/emoon/rust_minifb). The relay in `relay/` is a Cloudflare Worker; the wizard in `site/` is published by GitHub Pages. How the screen is laid out is in [DESIGN.md](DESIGN.md).

The faces are Gallant (from FreeBSD), Spleen and Terminus; their licences are in `crates/ok-gfx/fonts`, and every notice is on the `notices` page inside ok.
