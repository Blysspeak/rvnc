//! Wayland/Hyprland backend: the phone is a real extra monitor, not a nested Xephyr window.
//!
//! Flow: `hyprctl output create headless` makes a virtual output sized to the phone,
//! whatever is hosted on it is captured by wf-recorder (one session per phone connection,
//! so the stream always starts with SPS/PPS) and served as raw H.264 over ADB reverse.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const OUTPUT: &str = "rvnc-phone";
const PID_DIR: &str = "/tmp/rvnc";
const SESSION_SH: &str = "/tmp/rvnc/session.sh";

pub fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").map(|v| !v.is_empty()).unwrap_or(false)
}

fn save_pid(name: &str, pid: u32) {
    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(format!("{}/{}", PID_DIR, name), pid.to_string());
}

/// Run a `hyprctl eval` Lua snippet and return an error string when Hyprland refuses it.
fn eval(lua: &str) -> Result<(), String> {
    let out = Command::new("hyprctl")
        .args(["eval", lua])
        .output()
        .map_err(|e| format!("hyprctl: {}", e))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && !stderr.contains("error:") {
        Ok(())
    } else {
        Err(stderr.trim().to_string())
    }
}

/// Hyprland 0.56 dropped `hyprctl keyword` for the Lua parser: an output is configured
/// with `hl.monitor({...})` instead of a `monitor = name,res,x,y,scale` keyword.
///
/// `scale` is the Android density / 160: the panel is 1080x2400 at 480 dpi (3x), so at
/// scale 1 every UI element renders three times smaller on the phone than on a desktop.
/// The output itself is created by session.sh when the phone connects, not here.
fn monitor_lua(width: u32, height: u32, scale: f32) -> String {
    format!(
        "hl.monitor({{ output = \"{}\", mode = \"{}x{}\", position = \"auto\", scale = {}, disabled = false }})",
        OUTPUT, width, height, scale
    )
}

/// `monitors all` also lists disabled outputs: a disabled orphan still has to be found.
fn has_output(monitors_json: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(monitors_json)
        .ok()
        .and_then(|v| {
            v.as_array().map(|ms| {
                ms.iter()
                    .any(|m| m.get("name").and_then(|n| n.as_str()) == Some(OUTPUT))
            })
        })
        .unwrap_or(false)
}

pub fn output_exists() -> bool {
    Command::new("hyprctl")
        .args(["monitors", "all", "-j"])
        .output()
        .map(|o| has_output(&o.stdout))
        .unwrap_or(false)
}

pub fn remove_output() {
    let _ = Command::new("hyprctl")
        .args(["output", "remove", OUTPUT])
        .output();
}

/// Move a window to the phone output. `hyprctl` dispatchers only run through
/// `hl.dispatch(hl.dsp.*)`, and windows have to be passed as HL.Window objects.
fn move_window_script(predicate: &str) -> String {
    format!(
        "for _,w in ipairs(hl.get_windows()) do if {} then return hl.dispatch(hl.dsp.window.move({{window = w, monitor = \"{}\"}})) end end",
        predicate, OUTPUT
    )
}

pub fn move_pid_to_output(pid: u32) -> Result<(), String> {
    eval(&move_window_script(&format!("w.pid == {}", pid)))
}

pub fn move_class_to_output(class: &str) -> Result<(), String> {
    eval(&move_window_script(&format!("w.class == \"{}\"", class)))
}

fn hypr_json(args: &[&str]) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn monitor_id(name: &str) -> Option<i64> {
    hypr_json(&["monitors", "-j"])?
        .as_array()?
        .iter()
        .find(|m| m.get("name").and_then(|n| n.as_str()) == Some(name))
        .and_then(|m| m.get("id")?.as_i64())
}

/// `hyprctl eval` prints "ok" whatever the script returned, so the only honest check is the
/// client list: monitor id the window sits on and its fullscreen state (2 = fullscreen).
fn client_state(pid: u32) -> Option<(i64, u32)> {
    let clients = hypr_json(&["clients", "-j"])?;
    let c = clients
        .as_array()?
        .iter()
        .find(|c| c.get("pid").and_then(|p| p.as_u64()) == Some(pid as u64))?;
    Some((
        c.get("monitor")?.as_i64()?,
        c.get("fullscreen")?.as_u64()? as u32,
    ))
}

/// Launch the panel command and put its window on the phone output.
/// The window only exists once the app has mapped it, so poll for a while.
/// `class_fallback` is for apps that fork (`rvnc open foot` -> child window); the dashboard
/// itself must be matched by pid only: an egui Wayland surface has an empty class.
pub fn launch_panel(cmd: &[String], class_fallback: bool) -> Result<u32, String> {
    if cmd.is_empty() {
        return Err("empty panel command".to_string());
    }

    let child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {}", cmd[0], e))?;
    let pid = child.id();
    save_pid("panel", pid);

    let want = monitor_id(OUTPUT).ok_or_else(|| format!("output {} not found", OUTPUT))?;
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let _ = move_pid_to_output(pid);
        if client_state(pid).map(|(mon, _)| mon == want).unwrap_or(false) {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    if !class_fallback {
        return Err(format!("pid {} never reached output {}", pid, OUTPUT));
    }
    // The app may fork (foot -> child), so fall back to its window class.
    let class = Path::new(&cmd[0])
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let _ = move_class_to_output(&class);
    thread::sleep(Duration::from_millis(500));
    Ok(pid)
}

/// Moving and fullscreening a window makes Hyprland focus it, which would yank Vlad's focus off
/// his own monitor. `rvnc panel` records the focused monitor first and puts the focus back after.
pub fn focused_monitor() -> Option<String> {
    hypr_json(&["monitors", "-j"])?
        .as_array()?
        .iter()
        .find(|m| m.get("focused").and_then(|f| f.as_bool()) == Some(true))?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

pub fn focus_monitor(name: &str) -> Result<(), String> {
    eval(&format!(
        "return hl.dispatch(hl.dsp.focus({{ monitor = \"{}\" }}))",
        name
    ))
}

fn wait_fullscreen(pid: u32, tries: usize) -> bool {
    for _ in 0..tries {
        if client_state(pid).map(|(_, fs)| fs == 2).unwrap_or(false) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Fullscreen the moved window. `hyprctl eval` answers "ok" even when no window matched, so the
/// result is only accepted after reading the `fullscreen` flag back from `hyprctl clients -j`.
/// A surface with an empty Wayland class can only be found by pid, with the monitor as fallback.
pub fn fullscreen_pid(pid: u32) -> Result<(), String> {
    let by_pid = format!(
        "for _,w in ipairs(hl.get_windows()) do if w.pid == {} then return hl.dispatch(hl.dsp.window.fullscreen({{window = w, action = \"set\", mode = \"fullscreen\"}})) end end",
        pid
    );
    eval(&by_pid)?;
    if wait_fullscreen(pid, 20) {
        return Ok(());
    }

    // The window is the only thing `rvnc panel` just put on the phone output.
    let by_monitor = format!(
        "for _,w in ipairs(hl.get_windows()) do if w.monitor.name == \"{}\" then return hl.dispatch(hl.dsp.window.fullscreen({{window = w, action = \"set\", mode = \"fullscreen\"}})) end end",
        OUTPUT
    );
    eval(&by_monitor)?;
    if wait_fullscreen(pid, 20) {
        return Ok(());
    }
    Err(format!("pid {} did not become fullscreen", pid))
}

/// Cursor position before the panel is launched, so the pointer does not stay parked on the
/// headless output and get captured into the stream.
pub fn cursor_pos() -> Option<(f64, f64)> {
    let out = Command::new("hyprctl").arg("cursorpos").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (x, y) = text.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

pub fn cursor_move(x: f64, y: f64) -> Result<(), String> {
    eval(&format!(
        "return hl.dispatch(hl.dsp.cursor.move({{x = {}, y = {}}}))",
        x, y
    ))
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// One phone connection = one session. The output, the recorder and the panel are created when
/// ncat accepts the phone and torn down by the EXIT trap when the connection ends: a headless
/// output nobody records is rendered in a busy loop by Hyprland on NVIDIA and freezes the desktop.
///
/// The recorder starts before the panel, so the output never sits without a frame consumer while
/// the panel takes its seconds to map, and the watchdog's "no recorder" rule never fires on a
/// healthy session. Each connection gets its own wf-recorder, so the first bytes the phone sees
/// are Annex-B SPS/PPS + IDR. stdout is the phone socket: nothing else may be written to it.
fn session_script(rvnc: &str, fps: u32, quality: u32, width: u32, height: u32, scale: f32) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -u
D={dir}
F="$D/one-$$.fifo"
mkdir -p "$D"
# A reconnect waits here until the previous session has removed its output.
exec 9>"$D/session.lock"
flock -w 20 9 || exit 1

PIDS=()
cleanup() {{
  if ((${{#PIDS[@]}})); then kill "${{PIDS[@]}}" 2>/dev/null; fi
  wait 2>/dev/null
  if [ -f "$D/panel" ]; then kill "$(cat "$D/panel")" 2>/dev/null; rm -f "$D/panel"; fi
  hyprctl output remove {out} >/dev/null 2>&1
  rm -f "$F"
}}
trap cleanup EXIT
trap exit TERM INT HUP

hyprctl output create headless {out} >/dev/null 2>&1
hyprctl eval '{lua}' >/dev/null 2>&1
mkfifo "$F"
wf-recorder -o {out} -m h264 -c h264_nvenc -D \
  -p preset=p1 -p tune=ull -p cq={quality} -p g=30 -b 0 -r {fps} -f "$F" </dev/null >/dev/null 2>&1 &
PIDS+=("$!")
cat "$F" </dev/null &
PIDS+=("$!")
# The phone only reads; EOF on its side of the socket means it hung up.
cat <&0 >/dev/null &
PIDS+=("$!")
{rvnc} panel </dev/null >/dev/null 2>&1
# Whichever ends first (recorder, video pipe, phone) ends the session.
wait -n "${{PIDS[@]}}"
"#,
        dir = PID_DIR,
        out = OUTPUT,
        lua = monitor_lua(width, height, scale),
        rvnc = sh_quote(rvnc),
        fps = fps,
        quality = quality
    )
}

pub fn start_stream(port: u16, fps: u32, quality: u32, width: u32, height: u32, scale: f32) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current executable: {}", e))?;
    let script = session_script(&exe.to_string_lossy(), fps, quality, width, height, scale);
    let _ = fs::create_dir_all(PID_DIR);
    fs::write(SESSION_SH, script).map_err(|e| format!("{}: {}", SESSION_SH, e))?;
    let _ = Command::new("chmod").args(["+x", SESSION_SH]).output();

    let child = Command::new("ncat")
        .args(["-lk", &port.to_string(), "--sh-exec", SESSION_SH])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ncat: {}", e))?;
    save_pid("stream", child.id());

    // Spawned after the stream pid is saved: the watchdog lives while that file exists.
    let watchdog = Command::new(&exe)
        .arg("watchdog")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("watchdog: {}", e))?;
    save_pid("watchdog", watchdog.id());
    Ok(child.id())
}

/// How long the output may exist without a wf-recorder before the watchdog removes it.
const ORPHAN_GRACE: Duration = Duration::from_secs(3);
const WATCHDOG_EVERY: Duration = Duration::from_secs(2);

/// Orphan guard decision, independent of how a session ended (ncat killed, session.sh killed):
/// output present and no recorder for longer than the grace → remove.
#[derive(Default)]
struct OrphanClock {
    since: Option<Instant>,
}

impl OrphanClock {
    /// `true` means: remove the output (and the panel) now.
    fn tick(&mut self, output: bool, recorder: bool, now: Instant) -> bool {
        if !output || recorder {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) > ORPHAN_GRACE {
            self.since = None;
            return true;
        }
        false
    }
}

fn recorder_running() -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("wf-recorder -o {}", OUTPUT)])
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `rvnc watchdog`: runs while the stream is up, `rvnc stop` kills it.
pub fn run_watchdog() {
    let mut clock = OrphanClock::default();
    while stream_running() {
        if clock.tick(output_exists(), recorder_running(), Instant::now()) {
            kill_pid("panel");
            remove_output();
        }
        thread::sleep(WATCHDOG_EVERY);
    }
}

fn kill_pid(name: &str) -> bool {
    if let Ok(pid) = fs::read_to_string(format!("{}/{}", PID_DIR, name)) {
        let _ = Command::new("kill").arg(pid.trim()).output();
        let _ = fs::remove_file(format!("{}/{}", PID_DIR, name));
        return true;
    }
    false
}

pub fn stop() {
    kill_pid("watchdog");
    kill_pid("panel");
    if kill_pid("stream") {
        let _ = Command::new("pkill").args(["-f", "ncat -lk"]).output();
    }
    let _ = Command::new("pkill").args(["-f", "wf-recorder -o rvnc-phone"]).output();
    let _ = Command::new("rm").args(["-rf", "/tmp/rvnc"]).output();
}

pub fn stream_running() -> bool {
    fs::read_to_string(format!("{}/stream", PID_DIR)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> String {
        session_script("/opt/it's/rvnc", 60, 18, 2400, 1080, 3.0)
    }

    fn pos(s: &str, needle: &str) -> usize {
        s.find(needle).unwrap_or_else(|| panic!("missing {:?} in script", needle))
    }

    #[test]
    fn session_chain_is_linear() {
        let s = script();
        let lock = pos(&s, "flock -w 20 9 || exit 1");
        let trap = pos(&s, "trap cleanup EXIT");
        let create = pos(&s, "hyprctl output create headless rvnc-phone");
        let mode = pos(&s, r#"hl.monitor({ output = "rvnc-phone", mode = "2400x1080", position = "auto", scale = 3, disabled = false })"#);
        let rec = pos(&s, "wf-recorder -o rvnc-phone");
        let panel = pos(&s, "panel </dev/null >/dev/null 2>&1");
        let wait = pos(&s, r#"wait -n "${PIDS[@]}""#);
        // The trap must not exist before the lock: a waiting reconnect would remove the live output.
        assert!(lock < trap && trap < create && create < mode && mode < rec && rec < panel && panel < wait);
    }

    #[test]
    fn cleanup_tears_everything_down() {
        let s = script();
        let start = pos(&s, "cleanup() {");
        let end = start + pos(&s[start..], "\n}\n");
        let body = &s[start..end];
        for step in [r#"kill "${PIDS[@]}""#, r#"kill "$(cat "$D/panel")""#, "hyprctl output remove rvnc-phone", r#"rm -f "$F""#] {
            assert!(body.contains(step), "cleanup lacks {:?}", step);
        }
    }

    #[test]
    fn nothing_but_video_reaches_the_socket() {
        for line in script().lines().filter(|l| l.starts_with("hyprctl") || l.contains(" panel ")) {
            assert!(line.contains(">/dev/null 2>&1"), "writes to the phone socket: {}", line);
        }
    }

    #[test]
    fn rvnc_path_is_shell_quoted() {
        assert!(script().contains(r"'/opt/it'\''s/rvnc' panel"));
    }

    #[test]
    fn session_script_passes_shellcheck() {
        use std::io::Write;
        let Ok(mut child) = Command::new("shellcheck")
            .args(["-s", "bash", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        else {
            eprintln!("shellcheck not installed, skipped");
            return;
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script().as_bytes()).expect("write script");
        }
        let out = child.wait_with_output().expect("shellcheck output");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }

    #[test]
    fn monitors_json_parsing() {
        let live = br#"[{"id":0,"name":"DP-1"},{"id":1,"name":"rvnc-phone","disabled":true}]"#;
        assert!(has_output(live));
        assert!(!has_output(br#"[{"id":0,"name":"DP-1","description":"rvnc-phone"}]"#));
        assert!(!has_output(br#"[]"#));
        assert!(!has_output(b"Hyprland IPC didn't respond"));
        assert!(!has_output(b""));
    }

    #[test]
    fn watchdog_removes_orphan_after_grace() {
        let t0 = Instant::now();
        let mut c = OrphanClock::default();
        assert!(!c.tick(true, false, t0));
        assert!(!c.tick(true, false, t0 + Duration::from_secs(2)));
        assert!(!c.tick(true, false, t0 + ORPHAN_GRACE));
        assert!(c.tick(true, false, t0 + ORPHAN_GRACE + Duration::from_millis(1)));
        // Counting starts over after a removal.
        assert!(!c.tick(true, false, t0 + Duration::from_secs(4)));
    }

    #[test]
    fn watchdog_leaves_healthy_or_absent_output_alone() {
        let t0 = Instant::now();
        let mut c = OrphanClock::default();
        assert!(!c.tick(true, false, t0));
        // A recorder showing up resets the clock.
        assert!(!c.tick(true, true, t0 + Duration::from_secs(2)));
        assert!(!c.tick(true, false, t0 + Duration::from_secs(4)));
        assert!(!c.tick(true, false, t0 + Duration::from_secs(6)));
        assert!(c.tick(true, false, t0 + Duration::from_secs(8)));
        // No output: nothing to remove, however long it lasts.
        let mut c = OrphanClock::default();
        assert!(!c.tick(false, false, t0));
        assert!(!c.tick(false, false, t0 + Duration::from_secs(60)));
    }
}
