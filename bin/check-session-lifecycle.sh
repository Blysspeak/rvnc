#!/usr/bin/env bash
# Checks on a live Hyprland that the phone output dies with its connection.
#
# Run it yourself, phone app closed (a connected phone holds the session lock):
#   bin/check-session-lifecycle.sh            # uses target/release/rvnc, port 8800
#   RVNC=/path/to/rvnc PORT=8800 bin/check-session-lifecycle.sh
#
# 1. Hyprland main-thread CPU before.
# 2. A fake phone (ncat --recv-only) connects -> session.sh creates rvnc-phone + wf-recorder.
# 3. Drop: wf-recorder is killed. After 5 s rvnc-phone must be gone and Hyprland < 15% CPU.
# 4. Reconnect: the output comes back; the fake phone hangs up; after 5 s it is gone again.
# The trap removes rvnc-phone (and stops rvnc if this script started it) whatever happens.
set -u
OUT=rvnc-phone
PORT=${PORT:-8800}
RVNC=${RVNC:-$(cd "$(dirname "$0")/.." && pwd)/target/release/rvnc}
CPU_MAX=15
CLIENT=
STARTED=0
FAILS=0

cleanup() {
  if [ -n "$CLIENT" ]; then kill "$CLIENT" 2>/dev/null; fi
  if [ "$STARTED" = 1 ]; then "$RVNC" stop >/dev/null 2>&1; fi
  hyprctl output remove "$OUT" >/dev/null 2>&1
  if output_present; then echo "WARN: $OUT still present after cleanup"; fi
}
trap cleanup EXIT
trap exit INT TERM HUP

output_present() {
  hyprctl monitors all -j | grep -Eq "\"name\": *\"$OUT\""
}

recorder_running() {
  pgrep -f "wf-recorder -o $OUT" >/dev/null
}

output_gone() { ! output_present; }
recorder_gone() { ! recorder_running; }
cpu_ok() { [ "$CPU" != "?" ] && [ "$CPU" -lt "$CPU_MAX" ]; }

# CPU% of Hyprland's main thread (tid == pid) over one second.
main_thread_cpu() {
  local pid a b
  pid=$(pgrep -xo Hyprland) || { echo "?"; return; }
  a=$(awk '{ sub(/^.*\) /, ""); print $12 + $13 }' "/proc/$pid/task/$pid/stat")
  sleep 1
  b=$(awk '{ sub(/^.*\) /, ""); print $12 + $13 }' "/proc/$pid/task/$pid/stat")
  echo $(( (b - a) * 100 / $(getconf CLK_TCK) ))
}

# check <label> <command> [args...]
check() {
  local label=$1; shift
  if "$@"; then echo "  ok   $label"; else echo "  FAIL $label"; FAILS=$((FAILS + 1)); fi
}

connect() {
  ncat --recv-only 127.0.0.1 "$PORT" >/dev/null 2>&1 &
  CLIENT=$!
  for _ in $(seq 30); do recorder_running && return 0; sleep 0.5; done
  return 1
}

[ -x "$RVNC" ] || { echo "FAIL: $RVNC not found (cargo build --release)"; exit 1; }
command -v ncat >/dev/null || { echo "FAIL: ncat not installed"; exit 1; }
if output_present; then echo "FAIL: $OUT exists before the test (live session or orphan); the trap removes it now"; exit 1; fi

echo "Hyprland main thread before: $(main_thread_cpu)%"

if [ ! -f /tmp/rvnc/session.sh ]; then
  echo "rvnc not running, starting it (phone on USB)..."
  STARTED=1
  "$RVNC"
  [ -f /tmp/rvnc/session.sh ] || { echo "FAIL: rvnc did not start"; exit 1; }
fi

echo "== session"
check "fake phone got a wf-recorder" connect
check "$OUT created on connect" output_present

echo "== drop: kill wf-recorder"
pkill -f "wf-recorder -o $OUT"
sleep 5
check "$OUT removed within 5 s" output_gone
check "no wf-recorder left" recorder_gone
CPU=$(main_thread_cpu)
echo "  Hyprland main thread after drop: $CPU%"
check "Hyprland main thread < $CPU_MAX%" cpu_ok
kill "$CLIENT" 2>/dev/null; CLIENT=

echo "== reconnect, then the phone hangs up"
check "reconnect got a wf-recorder" connect
check "$OUT recreated on reconnect" output_present
kill "$CLIENT" 2>/dev/null; CLIENT=
sleep 5
check "$OUT removed after hangup" output_gone
check "no wf-recorder left after hangup" recorder_gone

if [ "$FAILS" = 0 ]; then echo "PASS"; else echo "FAIL ($FAILS checks)"; exit 1; fi
