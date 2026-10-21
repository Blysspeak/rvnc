# rVNC

**Use your Android phone as a second monitor over USB — GPU-accelerated, near real-time.**

rVNC streams your Linux desktop to an Android phone using H.264 hardware encoding (VAAPI) over USB. It creates an isolated virtual display via Xephyr, encodes frames on the GPU, and sends raw H.264 NAL units directly to the phone's hardware decoder (MediaCodec). No Wi-Fi required.

## Features

- **H.264 VAAPI** — GPU-accelerated encoding, 60fps at ~2Mbps
- **Near real-time** — raw NAL units over TCP, direct MediaCodec decoding (scrcpy-inspired)
- **USB streaming** — ADB reverse tunnel, zero network configuration
- **Isolated display** — Xephyr virtual screen, doesn't affect your main desktop
- **Mirror mode** — optionally mirror your main screen instead
- **Auto phone detection** — reads phone resolution via ADB, adapts automatically
- **GUI + CLI** — `rvnc-gui` (egui, Tiger theme) for visual control, `rvnc` for scripting
- **One-click install** — handles all dependencies, binaries, desktop entry, and APK

## Architecture

```
┌─────────────┐  x11grab   ┌──────────┐  H.264   ┌─────────────┐
│   Xephyr    │ ──────────►│  ffmpeg  │ ────────► │  ncat TCP   │
│ (virtual X) │            │  VAAPI   │  raw NAL  │  :8800      │
└─────────────┘            └──────────┘           └──────┬──────┘
                                                         │ ADB reverse
                                                  ┌──────▼──────┐
                                                  │ Android App │
                                                  │ MediaCodec  │
                                                  │ → Surface   │
                                                  └─────────────┘
```

## Requirements

- **Linux** — X11, Arch-based distro (installer uses pacman)
- **AMD/Intel GPU** with VAAPI support (check with `vainfo`)
- **Android phone** with USB debugging enabled
- **USB cable**

## Quick Start

```bash
git clone https://github.com/blysspeak/rvnc.git
cd rvnc
chmod +x install.sh
./install.sh
```

The installer will:
1. Install system dependencies (ffmpeg, Xephyr, openbox, ncat, libva-utils)
2. Copy `rvnc` and `rvnc-gui` to `~/.local/bin/`
3. Create a `.desktop` file (shows up in rofi / app launchers)
4. If a phone is connected — install the viewer APK and configure ADB

## Usage

### GUI

```bash
rvnc-gui
```

- Select bspwm desktop (5–10) for the virtual display
- Adjust FPS and quality
- Start/stop streaming
- Open apps on the phone screen (firefox, brave, etc.)
- Real-time status log with timestamps

### CLI

```bash
rvnc                        # start isolated display + stream
rvnc --mirror               # mirror main screen
rvnc --fps 30 --quality 25  # lower quality for weaker GPU
rvnc open firefox            # open app on phone display
rvnc stop                    # stop everything
rvnc status                  # show current state
```

### On your phone

1. Enable **USB debugging** in Developer Options
2. Connect phone via USB cable
3. Run `rvnc` or `rvnc-gui` on your PC
4. Open the **rVNC** app — it auto-connects to `127.0.0.1:8800`

## Building from Source

### Server + GUI (Rust)

```bash
cd server
cargo build --release
# Binaries: target/release/rvnc, target/release/rvnc-gui
```

### Android Viewer

```bash
cd android
export ANDROID_HOME=~/Android/Sdk
gradle assembleDebug
# APK: app/build/outputs/apk/debug/app-debug.apk
```

## Project Structure

```
rvnc/
├── server/              # Rust server + GUI
│   └── src/
│       ├── main.rs          # CLI: Xephyr, openbox, ffmpeg, ncat, ADB
│       ├── gui.rs           # egui control panel (Tiger palette)
│       ├── server.rs        # VNC/RFB protocol (legacy, for fallback)
│       ├── capture.rs       # X11 screen capture
│       └── rfb.rs           # RFB protocol types
├── android/             # Android viewer app
│   └── app/src/main/java/com/blyss/rvnc/
│       └── MainActivity.java   # MediaCodec H.264 decoder + SurfaceView
├── bin/                 # Pre-compiled binaries (x86_64)
│   ├── rvnc
│   ├── rvnc-gui
│   └── app-debug.apk
├── install.sh           # One-click installer
└── README.md
```

## How it works

1. **rvnc** starts Xephyr (virtual X display) with your phone's resolution
2. Openbox runs as window manager inside Xephyr
3. ffmpeg captures Xephyr via x11grab, encodes H.264 with VAAPI (GPU)
4. Raw H.264 stream is piped to ncat which serves it on TCP :8800
5. ADB reverse forwards port 8800 to the phone
6. Android app connects to `127.0.0.1:8800`, parses NAL units, feeds MediaCodec
7. Hardware decoder renders frames to SurfaceView — near real-time

## Limits and the output lifecycle

The phone is a real extra monitor: a Wayland session creates a headless output sized to
the phone, captures it with `wf-recorder`, and serves the H.264 stream over ADB reverse.
Every resource in that chain is bounded, because a stream that misbehaves must not take
the machine down with it.

| Limit | Value | Why |
|---|---|---|
| Capture rate | **≤ 60 fps** | `--fps` is a wish; it is clamped to `1..=60` (default 60) before it reaches the encoder. |
| Output create/remove | **≤ once per 5 s** | One action per gap, shared across the session, the fuse and the watchdog. |
| Acquire retry | **1 s → 30 s**, doubling | The pause between attempts doubles; a once-a-second retry burns a core for nothing. |
| Acquire fuse | **5 failures / 2 min** | A verdict, not a loop: the output is taken down and the process exits non-zero. |
| Teardown order | **capture → VNC → output** | The encoder is stopped and reaped first, then the socket side, then the output. |

The output lives for exactly one phone connection. `session.sh` creates it through
`rvnc acquire-output`, which owns the backoff ladder and the fuse; cleanup takes it down
through `rvnc release-output`, which obeys the same 5-second gate. Nothing creates or
removes the output by calling `hyprctl` directly — that is the rule the 5-second gate and
the fuse are built on.

`contrib/systemd/rvnc.service` is the packaging half of the same guard and is **not
installed by `install.sh`**: a user unit with `CPUQuota=25%`, `MemoryHigh=512M`,
`MemoryMax=768M`, `Nice=10`, `Restart=on-failure`, `RestartSec=30`, and
`StartLimitBurst=3` per 300 s. The capture process runs inside the service's cgroup, so
those ceilings cover the encoder too, and `KillMode=control-group` takes it down with the
service instead of orphaning it.

## License

MIT
