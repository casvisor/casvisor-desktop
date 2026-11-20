# Casvisor Desktop

Casvisor Desktop is the remote desktop client of [Casvisor](https://github.com/casvisor/casvisor). Install it on a computer and the computer can be controlled from Casvisor, with Casvisor's sign-in, permissions, approvals, session recording and audit log. It is one program for both sides: the computer being controlled runs it, and the people controlling it use Casvisor in the browser.

It connects out to Casvisor over HTTPS, so the computer needs no public IP address and no open port. Every session belongs to a Casvisor user, and unless the computer is set to unattended, the person in front of it is asked to allow the session and sees a bar on top of the screen while it lasts.

## Status

- Windows: screen capture (DXGI Desktop Duplication), H.264 encoding (Media Foundation), mouse and keyboard, text clipboard, multiple displays, and as a Windows service also the lock screen, the sign-in screen, UAC prompts and Ctrl+Alt+Del.
- Viewer: Casvisor's web page, which decodes the video with WebCodecs.
- Not yet: hardware encoders, file transfer, audio, macOS, Linux and Android, and controlling other computers from this app.

## How it works

1. In Casvisor, add a network gateway of the type **Casvisor Desktop device** and generate its token.
2. In Casvisor Desktop, enter Casvisor's address, the token and Casvisor's host key fingerprint shown with the token.
3. In Casvisor, add an asset with the protocol **Desktop** that uses the device, and give users the permission to connect to it.

The protocol between Casvisor Desktop and Casvisor is in [docs/protocol.md](docs/protocol.md).

## Windows service

Without the service, Casvisor Desktop runs as the signed-in user, which Windows doesn't let see or control the lock screen, the sign-in screen or UAC prompts. Click **Install service** in the app, or run as an administrator:

```powershell
casvisor-desktop.exe --install-service
```

The service runs as SYSTEM and starts the part that connects to Casvisor and captures the screen in the console session, also as SYSTEM, so it follows the screen to the secure desktop. The app of the signed-in user still asks whether to allow a session and shows the bar on top. The settings move to `%ProgramData%\Casvisor Desktop`, which only administrators can read, so changing them needs an administrator, and the logs are in its `logs` folder. Installing also sets the `SoftwareSASGeneration` policy so that the service may send Ctrl+Alt+Del, and runs the app for everyone who signs in.

At the lock screen nobody can see the request to allow a session, so set the computer to unattended to reach it then.

```powershell
casvisor-desktop.exe --uninstall-service
```

## Development

Casvisor Desktop is written in Rust with [Tauri 2](https://tauri.app/), the UI in React.

```bash
npm install
npm run tauri dev
```

Build the installers, or with `-- --no-bundle` only `target/release/casvisor-desktop.exe`. Build with the Tauri CLI, not `cargo build`: without it the app loads its UI from the dev server at `localhost:1420` instead of the copy built into it.

```bash
npm run tauri build
```

Check screen capture and encoding without Casvisor, which records 3 seconds of the screen to an H.264 file:

```bash
target/release/casvisor-desktop --capture-test out.h264
```

## License

[Apache-2.0](LICENSE)
