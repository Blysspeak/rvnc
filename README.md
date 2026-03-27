# rvnc

**Use your Android phone as a second monitor over USB.**

rvnc is a lightweight, GPU-friendly VNC-based screen streaming solution. It creates a virtual X display using Xephyr, captures frames via X11 SHM, and streams them to an Android viewer app over ADB reverse TCP — no Wi-Fi needed.

<!-- ![screenshot](screenshots/demo.png) -->

## Features

- **Virtual display** — Xephyr-backed second screen, any resolution
- **USB streaming** — ADB reverse tunnel, zero network config
- **Low latency** — X11 SHM capture + zlib-compressed RFB protocol
- **GUI & CLI** — `rvnc-gui` (egui) for easy setup, `rvnc` for scripting
- **Window manager** — Openbox runs inside the virtual display
- **One-click install** — script handles deps, binaries, desktop entry, APK

## Architecture

```
┌─────────────┐    RFB/TCP     ┌──────────────┐
│  rvnc server│◄──────────────►│ Android app  │
│  (Rust)     │   :5900 (USB)  │ (Java/Canvas)│
└──────┬──────┘                └──────────────┘
       │ X11 SHM
┌──────▼──────┐
│   Xephyr    │
│  (virtual X)│
└─────────────┘
```

## Requirements

- Arch Linux (or Arch-based distro)
- Android phone with USB debugging enabled
- USB cable

## Quick Start

```bash
git clone https://github.com/user/rvnc.git
cd rvnc
chmod +x install.sh
./install.sh
```

The installer will:
1. Install system dependencies via pacman (ffmpeg, Xephyr, openbox, ncat, libva)
2. Copy `rvnc` and `rvnc-gui` to `~/.local/bin/`
3. Create a `.desktop` file (shows up in rofi / app launchers)
4. If a phone is connected via ADB — install the viewer APK and set up port forwarding

## Usage

### GUI

```bash
rvnc-gui
```

Launch the graphical interface to configure resolution, start/stop the server, and manage ADB connection.

### CLI

```bash
# Start server with default settings (1080x2400, display :1)
rvnc

# Custom resolution and display
rvnc --width 1080 --height 2400 --display :2

# Show all options
rvnc --help
```

### On your phone

1. Enable USB debugging in Developer Options
2. Connect phone via USB
3. Open the **rvnc** app — it connects to `localhost:5900` automatically
4. Drag windows to the virtual display from your main screen

## Building from Source

### Server (Rust)

```bash
cd server
cargo build --release
# Binaries: target/release/rvnc, target/release/rvnc-gui
```

### Android Viewer

Open `android/` in Android Studio and build, or:

```bash
cd android
./gradlew assembleDebug
# APK: app/build/outputs/apk/debug/app-debug.apk
```

## Project Structure

```
rvnc/
├── server/          # Rust server + GUI
│   └── src/
│       ├── main.rs      # CLI entry, Xephyr/openbox management
│       ├── gui.rs        # egui control panel
│       ├── server.rs     # RFB/VNC protocol server
│       ├── capture.rs    # X11 SHM screen capture
│       └── rfb.rs        # RFB protocol types
├── android/         # Android viewer app
│   └── app/src/main/java/com/blyss/rvnc/
│       ├── MainActivity.java
│       ├── RfbClient.java
│       └── VncView.java
├── bin/             # Pre-compiled binaries
│   ├── rvnc
│   ├── rvnc-gui
│   └── app-debug.apk
├── install.sh       # One-click installer
└── README.md
```

## License

MIT
