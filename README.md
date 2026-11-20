# Casvisor Desktop

Casvisor Desktop is the remote desktop client of [Casvisor](https://github.com/casvisor/casvisor). Install it on a computer and the computer can be controlled from Casvisor, with Casvisor's sign-in, permissions, approvals, session recording and audit log. It is one program for both sides: the computer being controlled runs it, and the people controlling it use Casvisor in the browser.

It connects out to Casvisor over HTTPS, so the computer needs no public IP address and no open port. Every session belongs to a Casvisor user, and unless the computer is set to unattended, the person in front of it is asked to allow the session and sees a bar on top of the screen while it lasts.

## Status

- Windows: screen capture (DXGI Desktop Duplication), H.264 encoding (Media Foundation), mouse and keyboard, text clipboard, multiple displays.
- Viewer: Casvisor's web page, which decodes the video with WebCodecs.
- Not yet: running as a Windows service (the lock screen and UAC prompts can't be captured yet), hardware encoders, file transfer, audio, macOS, Linux and Android, and controlling other computers from this app.

## How it works

1. In Casvisor, add a network gateway of the type **Casvisor Desktop device** and generate its token.
2. In Casvisor Desktop, enter Casvisor's address, the token and Casvisor's host key fingerprint shown with the token.
3. In Casvisor, add an asset with the protocol **Desktop** that uses the device, and give users the permission to connect to it.

The protocol between Casvisor Desktop and Casvisor is in [docs/protocol.md](docs/protocol.md).

## Development

Casvisor Desktop is written in Rust with [Tauri 2](https://tauri.app/), the UI in React.

```bash
npm install
npm run tauri dev
```

Build the installers:

```bash
npm run tauri build
```

Check screen capture and encoding without Casvisor, which records 3 seconds of the screen to an H.264 file:

```bash
cargo build --release
target/release/casvisor-desktop --capture-test out.h264
```

## License

[Apache-2.0](LICENSE)
