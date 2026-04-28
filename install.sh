#!/bin/bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"

info()  { echo -e "  ${GREEN}✓${NC} $1"; }
warn()  { echo -e "  ${YELLOW}!${NC} $1"; }
error() { echo -e "  ${RED}✗${NC} $1"; }
header() { echo -e "\n${BOLD}${CYAN}$1${NC}"; }

# ── Menu ─────────────────────────────────────────────────────────────

show_menu() {
    echo ""
    echo -e "${BOLD}  rVNC Installer${NC}"
    echo -e "  ${DIM}─────────────────────────────${NC}"
    echo ""
    echo -e "  ${CYAN}1${NC}  Установить всё (Linux + Android)"
    echo -e "  ${CYAN}2${NC}  Только Linux (rvnc + rvnc-gui)"
    echo -e "  ${CYAN}3${NC}  Только Android (APK через ADB)"
    echo -e "  ${CYAN}4${NC}  Удалить rVNC"
    echo -e "  ${CYAN}0${NC}  Выход"
    echo ""
    read -rp "  Выбор [1-4]: " choice
    echo ""
}

# ── Dependencies ─────────────────────────────────────────────────────

install_deps() {
    header "Проверка зависимостей"

    local DEPS=(ffmpeg Xephyr openbox ncat)
    local PKGS=(ffmpeg xorg-server-xephyr openbox nmap)
    local MISSING=()

    for i in "${!DEPS[@]}"; do
        if command -v "${DEPS[$i]}" &>/dev/null; then
            info "${DEPS[$i]}"
        else
            warn "${DEPS[$i]} не найден → ${PKGS[$i]}"
            MISSING+=("${PKGS[$i]}")
        fi
    done

    # VA-API
    if pacman -Qi libva &>/dev/null; then
        info "VA-API (libva)"
    else
        warn "libva не найден"
        MISSING+=("libva" "libva-utils")
    fi

    if [[ ${#MISSING[@]} -gt 0 ]]; then
        echo ""
        warn "Нужно установить: ${MISSING[*]}"
        read -rp "  Установить через pacman? [Y/n] " ans
        ans="${ans:-y}"
        if [[ "$ans" =~ ^[Yy]$ ]]; then
            sudo pacman -S --needed --noconfirm "${MISSING[@]}"
            info "Зависимости установлены"
        else
            error "Отменено"
            return 1
        fi
    else
        info "Все зависимости на месте"
    fi
}

# ── Linux install ────────────────────────────────────────────────────

install_linux() {
    header "Установка Linux"

    install_deps || return 1

    mkdir -p "$BIN_DIR"

    cp "$SCRIPT_DIR/bin/rvnc" "$BIN_DIR/rvnc"
    chmod +x "$BIN_DIR/rvnc"
    info "rvnc → $BIN_DIR/rvnc"

    cp "$SCRIPT_DIR/bin/rvnc-gui" "$BIN_DIR/rvnc-gui"
    chmod +x "$BIN_DIR/rvnc-gui"
    info "rvnc-gui → $BIN_DIR/rvnc-gui"

    # .desktop
    mkdir -p "$APP_DIR"
    cat > "$APP_DIR/rvnc.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=rVNC
Comment=Stream screen to phone
Exec=rvnc-gui
Icon=video-display
Terminal=false
Categories=Utility;Network;
Keywords=vnc;screen;phone;stream;
DESKTOP
    info "rvnc.desktop → rofi / app menu"

    # PATH check
    if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
        warn "$BIN_DIR не в PATH — добавьте в ~/.zshrc или ~/.bashrc:"
        echo -e "    ${DIM}export PATH=\"\$HOME/.local/bin:\$PATH\"${NC}"
    fi

    echo ""
    info "Linux установка завершена"
}

# ── Android install ──────────────────────────────────────────────────

install_android() {
    header "Установка Android"

    # Check ADB
    if ! command -v adb &>/dev/null; then
        error "adb не найден. Установите: sudo pacman -S android-tools"
        return 1
    fi
    info "adb найден"

    # Check phone
    if ! adb devices 2>/dev/null | grep -q "device$"; then
        error "Телефон не подключён или USB debugging не включён"
        echo ""
        echo -e "  ${DIM}1. Настройки → О телефоне → 7 тапов по номеру сборки${NC}"
        echo -e "  ${DIM}2. Для разработчиков → Отладка по USB → Вкл${NC}"
        echo -e "  ${DIM}3. Подключите USB и подтвердите на телефоне${NC}"
        return 1
    fi

    local phone_model
    phone_model=$(adb shell getprop ro.product.model 2>/dev/null | tr -d '\r')
    local phone_res
    phone_res=$(adb shell wm size 2>/dev/null | grep -oP '\d+x\d+' | tail -1)
    info "Телефон: $phone_model ($phone_res)"

    # Install APK
    local apk="$SCRIPT_DIR/bin/rvnc.apk"
    if [[ ! -f "$apk" ]]; then
        apk="$SCRIPT_DIR/bin/app-debug.apk"
    fi

    if [[ ! -f "$apk" ]]; then
        error "APK не найден в bin/"
        return 1
    fi

    info "Установка APK..."
    if adb install -r "$apk" 2>&1 | grep -q "Success"; then
        info "rVNC установлен на телефон"
    else
        error "Ошибка установки APK"
        return 1
    fi

    # ADB reverse
    adb reverse tcp:8800 tcp:8800
    info "ADB reverse tcp:8800 настроен"

    echo ""
    info "Android установка завершена"
    info "Откройте rVNC на телефоне после запуска rvnc на ПК"
}

# ── Uninstall ────────────────────────────────────────────────────────

uninstall() {
    header "Удаление rVNC"

    # Stop if running
    if command -v rvnc &>/dev/null; then
        rvnc stop 2>/dev/null || true
    fi

    # Linux
    rm -f "$BIN_DIR/rvnc" "$BIN_DIR/rvnc-gui"
    rm -f "$APP_DIR/rvnc.desktop"
    rm -rf /tmp/rvnc
    info "Linux бинарники и .desktop удалены"

    # Android
    if command -v adb &>/dev/null && adb devices 2>/dev/null | grep -q "device$"; then
        read -rp "  Удалить APK с телефона? [y/N] " ans
        if [[ "$ans" =~ ^[Yy]$ ]]; then
            adb uninstall com.blyss.rvnc 2>/dev/null && info "APK удалён" || warn "APK не найден на телефоне"
            adb reverse --remove-all 2>/dev/null
        fi
    fi

    echo ""
    info "rVNC удалён"
}

# ── Main ─────────────────────────────────────────────────────────────

show_menu

case "${choice:-1}" in
    1)
        install_linux
        install_android
        ;;
    2)
        install_linux
        ;;
    3)
        install_android
        ;;
    4)
        uninstall
        ;;
    0)
        echo "  Bye"
        exit 0
        ;;
    *)
        error "Неверный выбор"
        exit 1
        ;;
esac

echo ""
echo -e "  ${BOLD}Готово!${NC} Запуск: ${CYAN}rvnc-gui${NC} или ${CYAN}rvnc --help${NC}"
echo ""
