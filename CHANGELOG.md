# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `rvnc acquire-output` / `rvnc release-output`: the headless output is now acquired
  through an exponential backoff ladder (1 s → 30 s) with a fuse — five failures inside
  two minutes take the output down and exit non-zero instead of retrying forever.
- `contrib/systemd/rvnc.service`: a user unit template with `CPUQuota=25%`,
  `MemoryHigh=512M`, `MemoryMax=768M`, `Nice=10`, `Restart=on-failure`,
  `RestartSec=30` and `StartLimitBurst=3` per 300 s; the capture runs in the same cgroup.
- README section on limits and the output lifecycle.

### Changed
- The capture rate is clamped to `1..=15` fps: `--fps` can no longer hand the encoder a
  number the machine cannot afford.
- `hyprctl output create/remove` runs at most once per five seconds, across every path
  (session, fuse, watchdog).
- Teardown runs in a fixed order — capture, then VNC, then the output — and the encoder is
  reaped with `wait` instead of merely signalled.

## [v0.2.0] - 2026-04-28

### Added
- GPU-accelerated screen streaming to phone via USB (ffmpeg h264_vaapi)
- Virtual display via Xephyr + openbox window manager
- ADB reverse port forwarding for USB-only connection
- `--app` flag to launch an app on virtual display at startup
- `rvnc open <cmd>` subcommand to open apps after start
- Chromium-based browser support via separate `--user-data-dir`
- `--mirror` mode to stream main display instead of isolated one
- `--fps`, `--quality`, `--port` tuning flags
- `rvnc stop` and `rvnc status` subcommands
- GUI binary (`rvnc-gui`)
