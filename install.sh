#!/bin/bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"

info()  { echo -e "${GREEN}[+]${NC} $1"; }
warn()  { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; }

# ── Check & install dependencies ──────────────────────────────────────

DEPS=(ffmpeg xephyr openbox ncat adb)
PKGS=(ffmpeg xorg-server-xephyr openbox openbsd-netcat android-tools)
MISSING_PKGS=()

for i in "${!DEPS[@]}"; do
    cmd="${DEPS[$i]}"
    pkg="${PKGS[$i]}"
    # xephyr binary is actually Xephyr (capital X)
    if [[ "$cmd" == "xephyr" ]]; then
        cmd="Xephyr"
    fi
    if ! command -v "$cmd" &>/dev/null; then
        warn "$cmd not found (package: $pkg)"
        MISSING_PKGS+=("$pkg")
    else
        info "$cmd found"
    fi
done

# Check VA-API
if ! pacman -Qi libva &>/dev/null; then
    warn "libva (VA-API) not found"
    MISSING_PKGS+=("libva" "libva-utils")
else
    info "VA-API (libva) found"
fi

if [[ ${#MISSING_PKGS[@]} -gt 0 ]]; then
    echo ""
    warn "Missing packages: ${MISSING_PKGS[*]}"
    read -rp "Install with pacman? [Y/n] " ans
    ans="${ans:-y}"
    if [[ "$ans" =~ ^[Yy]$ ]]; then
        sudo pacman -S --needed --noconfirm "${MISSING_PKGS[@]}"
    else
        error "Cannot continue without dependencies"
        exit 1
    fi
fi

# ── Install binaries ─────────────────────────────────────────────────

mkdir -p "$BIN_DIR"

info "Installing rvnc to $BIN_DIR/rvnc"
cp "$SCRIPT_DIR/bin/rvnc" "$BIN_DIR/rvnc"
chmod +x "$BIN_DIR/rvnc"

info "Installing rvnc-gui to $BIN_DIR/rvnc-gui"
cp "$SCRIPT_DIR/bin/rvnc-gui" "$BIN_DIR/rvnc-gui"
chmod +x "$BIN_DIR/rvnc-gui"

# ── Create .desktop file ─────────────────────────────────────────────

mkdir -p "$APP_DIR"

cat > "$APP_DIR/rvnc.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=rvnc
Comment=Stream screen to phone via VNC
Exec=rvnc-gui
Icon=video-display
Terminal=false
Categories=Utility;Network;
Keywords=vnc;screen;phone;stream;
DESKTOP

info "Created $APP_DIR/rvnc.desktop"

# ── ADB: install APK & setup reverse ─────────────────────────────────

if command -v adb &>/dev/null && adb devices | grep -q "device$"; then
    info "Phone detected via ADB"

    read -rp "Install rvnc-viewer APK on phone? [Y/n] " ans
    ans="${ans:-y}"
    if [[ "$ans" =~ ^[Yy]$ ]]; then
        info "Installing APK..."
        adb install -r "$SCRIPT_DIR/bin/app-debug.apk"
    fi

    info "Setting up ADB reverse (tcp:5900 -> localhost:5900)"
    adb reverse tcp:5900 tcp:5900
else
    warn "No phone detected via ADB — skipping APK install"
    warn "Connect phone later and run: adb install bin/app-debug.apk && adb reverse tcp:5900 tcp:5900"
fi

# ── Done ──────────────────────────────────────────────────────────────

echo ""
info "Installation complete!"
info "Run 'rvnc-gui' or launch from rofi/app menu"
info "CLI usage: rvnc --help"
