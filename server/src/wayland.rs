//! Wayland/Hyprland backend: the phone is a real extra monitor, not a nested Xephyr window.
//!
//! Flow: `hyprctl output create headless` makes a virtual output sized to the phone,
//! whatever is hosted on it is captured by wf-recorder (one session per phone connection,
//! so the stream always starts with SPS/PPS) and served as raw H.264 over ADB reverse.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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
pub fn ensure_output(width: u32, height: u32) -> Result<(), String> {
    let _ = Command::new("hyprctl")
        .args(["output", "create", "headless", OUTPUT])
        .output();

    eval(&format!(
        "hl.monitor({{ output = \"{}\", mode = \"{}x{}\", position = \"auto\", scale = 1, disabled = false }})",
        OUTPUT, width, height
    ))
}

pub fn output_exists() -> bool {
    Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{}\"", OUTPUT)))
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

/// Launch the panel command and put its window on the phone output.
/// The window only exists once the app has mapped it, so poll for a while.
pub fn launch_panel(cmd: &[String]) -> Result<u32, String> {
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

    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        if move_pid_to_output(pid).is_ok() {
            return Ok(pid);
        }
    }
    // The app may fork (foot -> child), so fall back to its window class.
    let class = Path::new(&cmd[0])
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let _ = move_class_to_output(&class);
    Ok(pid)
}

fn session_script(port: u16, fps: u32, quality: u32) -> String {
    // Each phone connection gets its own wf-recorder, so the first bytes the phone
    // sees are Annex-B SPS/PPS + IDR — otherwise MediaCodec has nothing to prime with.
    format!(
        r#"#!/usr/bin/env bash
set -u
F=/tmp/rvnc/one-$$.fifo
mkdir -p /tmp/rvnc
mkfifo "$F"
wf-recorder -o {out} -m h264 -c h264_nvenc \
  -p preset=p1 -p tune=ull -p cq={quality} -b 0 -r {fps} -f "$F" >/dev/null 2>&1 &
W=$!
cat "$F"
kill "$W" 2>/dev/null
wait "$W" 2>/dev/null
rm -f "$F"
"#,
        out = OUTPUT,
        fps = fps,
        quality = quality
    )
}

pub fn start_stream(port: u16, fps: u32, quality: u32) -> Result<u32, String> {
    let _ = fs::create_dir_all(PID_DIR);
    fs::write(SESSION_SH, session_script(port, fps, quality))
        .map_err(|e| format!("{}: {}", SESSION_SH, e))?;
    let _ = Command::new("chmod").args(["+x", SESSION_SH]).output();

    let child = Command::new("ncat")
        .args(["-lk", &port.to_string(), "--sh-exec", SESSION_SH])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ncat: {}", e))?;
    save_pid("stream", child.id());
    Ok(child.id())
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
    if kill_pid("stream") {
        let _ = Command::new("pkill").args(["-f", "ncat -lk"]).output();
    }
    let _ = Command::new("pkill").args(["-f", "wf-recorder -o rvnc-phone"]).output();
    let _ = Command::new("rm").args(["-rf", "/tmp/rvnc"]).output();
}

pub fn stream_running() -> bool {
    fs::read_to_string(format!("{}/stream", PID_DIR)).is_ok()
}
