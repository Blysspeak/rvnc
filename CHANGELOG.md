# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v0.1.0] - 2026-04-28

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
