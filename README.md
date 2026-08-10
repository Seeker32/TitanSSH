# TitanSSH

[简体中文](README.zh-CN.md)

TitanSSH is a desktop app for working with Linux servers over SSH. Keep your server connections in one place, open terminal sessions in tabs, transfer files, and check the essentials without leaving the app.

## What you can do

- Save and organize server connections
- Sign in with a password or an SSH private key
- Open multiple terminal sessions at once
- Choose a terminal-only theme without changing the rest of the app
- Upload and download files with progress tracking
- View CPU, memory, disk, network activity, and server uptime

Passwords are stored in your operating system's secure keychain. Private keys stay on your computer; TitanSSH stores only their file paths.

## Get started

You will need Node.js 22.13 or newer, pnpm, and a current stable Rust toolchain. On macOS, also install Xcode Command Line Tools. Linux and Windows may need their usual C/C++ build tools.

```bash
pnpm install
pnpm tauri dev
```

The app will open in development mode. Add a connection from the sidebar, then double-click it to start a terminal session.

## Terminal themes

Open **Settings** in the sidebar to choose a terminal theme. TitanSSH includes Light, Dark, One Dark, Dracula, Solarized Light, and Solarized Dark. Your choice is saved locally and affects terminal content only, so the app's own light or dark appearance stays unchanged.

## Checks and builds

```bash
pnpm test
pnpm tauri build
```

The packaged app is written to `src-tauri/target/release/bundle/`.

## Troubleshooting

**Can't connect?** Confirm the server address, SSH service, firewall rules, username, and authentication details.

**Development app will not start?** Make sure Node.js, pnpm, Rust, and the platform build tools above are installed.

## License

MIT
