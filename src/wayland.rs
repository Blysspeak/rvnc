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

/// The capture ceiling: no path in this crate asks wf-recorder for more than this.
pub const MAX_CAPTURE_FPS: u32 = 15;

/// Clamp a requested frame rate into the range wf-recorder can be trusted with.
///
/// The flag is a wish, not a contract — `--fps 120` used to reach `-r 120` verbatim, and
/// the encoder answered by eating the GPU the incident of 21.10 left without memory.
pub fn capture_fps(requested: u32) -> u32 {
    requested.clamp(1, MAX_CAPTURE_FPS)
}

/// The first pause before asking for the output again.
pub const ACQUIRE_BASE_DELAY: Duration = Duration::from_secs(1);
/// No single pause in the ladder waits longer than this.
pub const ACQUIRE_MAX_DELAY: Duration = Duration::from_secs(30);
/// Failures inside [`ACQUIRE_FUSE_WINDOW`] that mean "this is not a slow daemon".
pub const ACQUIRE_FUSE_FAILURES: u32 = 5;
/// The window the fuse counts in.
pub const ACQUIRE_FUSE_WINDOW: Duration = Duration::from_secs(120);

/// What the acquisition ladder does after a failed attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum AcquireStep {
    /// Wait this long, then try again. Doubles each time, capped at [`ACQUIRE_MAX_DELAY`].
    RetryAfter(Duration),
    /// Stop asking: give up, take the output down and exit non-zero.
    GiveUp,
}

/// Pure acquisition policy: one decision per failure, no I/O and no clock.
///
/// The 2026-10-21 hang is what this encodes: a once-a-second retry pinned a core at ~99%
/// until kswapd gave up and the whole session died. So the ladder doubles instead of
/// repeating, and [`ACQUIRE_FUSE_FAILURES`] failures inside [`ACQUIRE_FUSE_WINDOW`] are a
/// verdict, not a reason to keep spinning. Failures spread wider than the window are a
/// slow daemon, not a broken one, and the ladder simply keeps going at its cap.
pub fn acquire_step(failures: u32, elapsed: Duration) -> AcquireStep {
    if failures >= ACQUIRE_FUSE_FAILURES && elapsed <= ACQUIRE_FUSE_WINDOW {
        return AcquireStep::GiveUp;
    }
    let doublings = failures.saturating_sub(1).min(31);
    let delay = ACQUIRE_BASE_DELAY
        .saturating_mul(1u32 << doublings)
        .min(ACQUIRE_MAX_DELAY);
    AcquireStep::RetryAfter(delay)
}

/// Ask Hyprland for the phone output until it is really there.
///
/// The old session script called `hyprctl output create` blind and moved on: when Hyprland
/// refused, wf-recorder died, the connection closed, and whatever restarted the session
/// asked again a second later. This loop owns that decision instead — backoff from
/// [`acquire_step`], and on [`AcquireStep::GiveUp`] the half-made output is removed and the
/// caller exits non-zero.
pub fn acquire_output(width: u32, height: u32, scale: f32) -> Result<(), String> {
    let started = Instant::now();
    let mut failures = 0u32;
    loop {
        if output_exists() {
            return eval(&monitor_lua(width, height, scale));
        }
        // The ladder can reach the create faster than the gate allows (1s, 2s, 4s);
        // the gate is the rule, so the attempt simply waits out the remainder.
        let now = now_unix();
        let last = last_output_action();
        if !output_action_allowed(last, now) {
            // A stamp from the future (clock stepped back) is waited out from zero rather
            // than trusted; `saturating_sub` keeps a backwards clock from panicking.
            let waited = now.saturating_sub(last.unwrap_or(now));
            thread::sleep(OUTPUT_ACTION_MIN_GAP.saturating_sub(Duration::from_secs(waited)));
        }
        stamp_output_action(now_unix());
        let attempt = Command::new("hyprctl")
            .args(["output", "create", "headless", OUTPUT])
            .output();
        let created = attempt.map(|o| o.status.success()).unwrap_or(false);
        // A created output is not a usable output yet: Hyprland publishes the monitor on
        // its own schedule, so the answer is the next `output_exists()`, not this status.
        if created && output_exists() {
            return eval(&monitor_lua(width, height, scale));
        }
        failures += 1;
        match acquire_step(failures, started.elapsed()) {
            AcquireStep::GiveUp => {
                release_output();
                return Err(format!(
                    "gave up on {} after {} attempts in {:?}: the output never appeared",
                    OUTPUT,
                    failures,
                    started.elapsed()
                ));
            }
            AcquireStep::RetryAfter(delay) => thread::sleep(delay),
        }
    }
}

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

/// How often the virtual output may be created or removed, whatever else happens.
///
/// This is a floor between ACTIONS, not a retry delay: the acquisition ladder already
/// paces its attempts, and this is the harder rule the whole process obeys — including
/// the watchdog, the fuse and a reconnect racing a teardown.
pub const OUTPUT_ACTION_MIN_GAP: Duration = Duration::from_secs(5);
/// Epoch seconds of the last create/remove, kept in a file because create and remove
/// happen in DIFFERENT processes (session.sh, the fuse, the watchdog).
const OUTPUT_ACTION_STAMP: &str = "/tmp/rvnc/output-action";

/// Pure rate decision over two epoch-second stamps: is a create/remove allowed now?
///
/// A stamp in the future (clock stepped back) is refused rather than trusted: the cost of
/// waiting is a few seconds, the cost of a create/remove storm is the machine.
pub fn output_action_allowed(last: Option<u64>, now: u64) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_sub(last) >= OUTPUT_ACTION_MIN_GAP.as_secs(),
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn last_output_action() -> Option<u64> {
    fs::read_to_string(OUTPUT_ACTION_STAMP).ok()?.trim().parse().ok()
}

fn stamp_output_action(now: u64) {
    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(OUTPUT_ACTION_STAMP, now.to_string());
}

/// Remove the output, but never faster than [`OUTPUT_ACTION_MIN_GAP`].
///
/// `false` means the gate refused: the output is still there and a later call will take it
/// down. Cleanup must not sleep for the gate — the phone is gone already.
pub fn release_output() -> bool {
    let now = now_unix();
    if !output_action_allowed(last_output_action(), now) {
        return false;
    }
    stamp_output_action(now);
    remove_output();
    true
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
REC=""
cleanup() {{
  # Order matters and is not accidental: capture → VNC → output.
  # 1. Stop the encoder first and REAP it: a frame written into a pipe nobody reads
  #    is what kept a dying session's process alive.
  if [ -n "$REC" ]; then kill "$REC" 2>/dev/null; wait "$REC" 2>/dev/null; fi
  # 2. Then the VNC side: the feed into the socket and the phone's own reader.
  if ((${{#PIDS[@]}})); then kill "${{PIDS[@]}}" 2>/dev/null; wait 2>/dev/null; fi
  if [ -f "$D/panel" ]; then kill "$(cat "$D/panel")" 2>/dev/null; rm -f "$D/panel"; fi
  # 3. The output goes last, through the rate gate: nothing may still be drawing on it.
  {rvnc} release-output >/dev/null 2>&1
  rm -f "$F"
}}
trap cleanup EXIT
trap exit TERM INT HUP

# The guard owns the retry ladder and the fuse: a blind create is what spun a core.
{rvnc} acquire-output --width {width} --height {height} --scale {scale} >/dev/null 2>&1 || exit 1
mkfifo "$F"
wf-recorder -o {out} -m h264 -c h264_nvenc -D \
  -p preset=p1 -p tune=ull -p cq={quality} -p g=30 -b 0 -r {fps} -f "$F" </dev/null >/dev/null 2>&1 &
REC=$!
cat "$F" </dev/null &
PIDS+=("$!")
# The phone only reads; EOF on its side of the socket means it hung up.
cat <&0 >/dev/null &
PIDS+=("$!")
{rvnc} panel </dev/null >/dev/null 2>&1
# Whichever ends first (recorder, video pipe, phone) ends the session. The recorder is
# not in PIDS any more — it is reaped by name in cleanup — so it is named here too.
wait -n "$REC" "${{PIDS[@]}}"
"#,
        dir = PID_DIR,
        out = OUTPUT,
        rvnc = sh_quote(rvnc),
        width = width,
        height = height,
        scale = scale,
        fps = capture_fps(fps),
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
        let create = pos(&s, "acquire-output --width 2400 --height 1080 --scale 3");
        let rec = pos(&s, "wf-recorder -o rvnc-phone");
        let panel = pos(&s, "panel </dev/null >/dev/null 2>&1");
        let wait = pos(&s, r#"wait -n "$REC" "${PIDS[@]}""#);
        // The trap must not exist before the lock: a waiting reconnect would remove the live output.
        assert!(lock < trap && trap < create && create < rec && rec < panel && panel < wait);
    }

    #[test]
    fn cleanup_tears_everything_down() {
        let s = script();
        let start = pos(&s, "cleanup() {");
        let end = start + pos(&s[start..], "\n}\n");
        let body = &s[start..end];
        for step in [r#"kill "${PIDS[@]}""#, r#"kill "$(cat "$D/panel")""#, "release-output", r#"rm -f "$F""#] {
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
    fn teardown_takes_capture_first_then_vnc_then_the_output() {
        let s = script();
        let start = pos(&s, "cleanup() {");
        let end = start + pos(&s[start..], "\n}\n");
        let body = &s[start..end];
        let capture = body
            .find(r#"kill "$REC""#)
            .unwrap_or_else(|| panic!("cleanup must stop the recorder first:\n{}", body));
        let vnc = body
            .find(r#"kill "${PIDS[@]}""#)
            .unwrap_or_else(|| panic!("cleanup must end the VNC feed second:\n{}", body));
        let output = body
            .find("release-output")
            .unwrap_or_else(|| panic!("cleanup must take the output down last:\n{}", body));
        assert!(
            capture < vnc && vnc < output,
            "teardown order is capture → VNC → output, not the other way around:\n{}",
            body
        );
        // Signalled is not stopped: the encoder has to be reaped before its pipe goes,
        // or a frame it was mid-write on is what keeps the session's process alive.
        assert!(
            body.contains(r#"wait "$REC""#),
            "the recorder must be waited for after the signal:\n{}",
            body
        );
    }

    #[test]
    fn capture_never_asks_for_more_than_fifteen_frames() {
        // wf-recorder on the headless output competes with every other client for the same
        // GPU; the phone cannot show more than 15 fps of a 6.6" panel usefully anyway.
        assert_eq!(capture_fps(60), 15);
        assert_eq!(capture_fps(120), 15);
        assert_eq!(capture_fps(15), 15);
        assert_eq!(capture_fps(8), 8);
        // Zero would make wf-recorder run flat out; one is the slowest honest rate.
        assert_eq!(capture_fps(0), 1);
        // The generated script carries the clamped number, never the flag value.
        let s = session_script("/opt/rvnc", 60, 18, 2400, 1080, 3.0);
        assert!(s.contains("-r 15"), "-r must carry the capped rate: {}", s.lines().find(|l| l.contains("wf-recorder")).unwrap_or(""));
        assert!(!s.contains("-r 60"), "the raw --fps value must not reach wf-recorder");
    }

    #[test]
    fn create_and_remove_never_run_more_than_once_per_five_seconds() {
        // Whatever happens — a reconnect storm, a fuse, a watchdog sweep — the virtual
        // output is touched at most once per gap. Hyprland handled each create/remove by
        // re-doing layout for every client, which is what ground the machine on 21.10.
        assert!(output_action_allowed(None, 1_000));
        assert!(output_action_allowed(Some(1_000), 1_005));
        assert!(!output_action_allowed(Some(1_000), 1_004));
        assert!(!output_action_allowed(Some(1_000), 1_001));
        // A clock that went backwards (or a stamp from the future) is not a free pass.
        assert!(!output_action_allowed(Some(2_000), 1_000));
    }

    #[test]
    fn output_acquisition_backs_off_and_then_gives_up() {
        let secs = Duration::from_secs;
        assert_eq!(acquire_step(1, secs(0)), AcquireStep::RetryAfter(secs(1)));
        assert_eq!(acquire_step(2, secs(1)), AcquireStep::RetryAfter(secs(2)));
        assert_eq!(acquire_step(3, secs(3)), AcquireStep::RetryAfter(secs(4)));
        assert_eq!(acquire_step(4, secs(7)), AcquireStep::RetryAfter(secs(8)));
        // The ladder is capped: no attempt waits longer than 30s, however many failed.
        assert_eq!(acquire_step(6, secs(200)), AcquireStep::RetryAfter(secs(30)));
        assert_eq!(acquire_step(20, secs(300)), AcquireStep::RetryAfter(secs(30)));
        // Five failures inside two minutes blow the fuse instead of spinning forever.
        assert_eq!(acquire_step(5, secs(31)), AcquireStep::GiveUp);
        // Further failures inside the window are the same verdict, not a taller ladder.
        assert_eq!(acquire_step(9, secs(60)), AcquireStep::GiveUp);
        // Five failures spread over more than two minutes are just a slow daemon.
        assert_eq!(acquire_step(5, secs(121)), AcquireStep::RetryAfter(secs(16)));
    }

    #[test]
    fn the_session_script_acquires_the_output_through_the_guard() {
        let s = script();
        assert!(
            s.contains("acquire-output"),
            "session.sh must ask the guard for the output, not call hyprctl blind"
        );
        assert!(
            !s.contains("hyprctl output create"),
            "a blind create is what left the retry loop spinning at 100% CPU"
        );
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
