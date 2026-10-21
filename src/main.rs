mod wayland;

use clap::{Parser, Subcommand};
use colored::*;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::fs;

#[derive(Parser)]
#[command(name = "rvnc", about = "Stream PC screen to phone via USB")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// FPS
    #[arg(short, long, default_value_t = 60)]
    fps: u32,

    /// Quality (1-51, lower = better)
    #[arg(short, long, default_value_t = 18)]
    quality: u32,

    /// Port
    #[arg(short, long, default_value_t = 8800)]
    port: u16,

    /// Mirror main screen instead of isolated display
    #[arg(short, long)]
    mirror: bool,

    /// App to open on virtual display after start
    #[arg(short, long, num_args = 1..)]
    app: Vec<String>,

    /// Wayland: UI scale override (default: phone density / 160)
    #[arg(long)]
    scale: Option<f32>,
}

#[derive(Subcommand)]
enum Commands {
    /// Stop all rvnc processes
    Stop,
    /// Show status
    Status,
    /// Open app on phone display
    Open {
        /// Command to run
        cmd: Vec<String>,
    },
    /// Run the dashboard on the phone display (default: rvnc-panel)
    Panel {
        /// Command to run (default: rvnc-panel next to this executable)
        cmd: Vec<String>,
    },
    /// Orphan guard for the phone output (started by `rvnc`, killed by `rvnc stop`)
    #[command(hide = true)]
    Watchdog,
    /// Take the phone output down through the rate gate (run by session.sh)
    #[command(hide = true)]
    ReleaseOutput,
    /// Acquire the phone output through the backoff guard (run by session.sh)
    #[command(hide = true)]
    AcquireOutput {
        /// Logical width of the phone output
        #[arg(long)]
        width: u32,
        /// Logical height of the phone output
        #[arg(long)]
        height: u32,
        /// UI scale
        #[arg(long)]
        scale: f32,
    },
}

const DISPLAY: &str = ":9";
const PID_DIR: &str = "/tmp/rvnc";

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Stop) => stop(),
        Some(Commands::Status) => status(),
        Some(Commands::Open { cmd }) => open_app(&cmd),
        Some(Commands::Panel { cmd }) => panel(cmd),
        Some(Commands::Watchdog) => wayland::run_watchdog(),
        // A refused release is not an error: the output outlives this call and a later
        // one takes it down. Cleanup must never fail because the gate said "not yet".
        Some(Commands::ReleaseOutput) => {
            wayland::release_output();
        }
        Some(Commands::AcquireOutput { width, height, scale }) => {
            if let Err(e) = wayland::acquire_output(width, height, scale) {
                eprintln!("{} {}", "✗".red(), e);
                // A non-zero exit is the contract with session.sh: the trap runs, the
                // output goes, and whatever restarted the session sees a real failure.
                std::process::exit(1);
            }
        }
        None => start(cli),
    }
}

const ZEN_FILE: &str = "/tmp/rvnc/zen_mode_before";

fn save_pid(name: &str, pid: u32) {
    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(format!("{}/{}", PID_DIR, name), pid.to_string());
}

fn adb_shell(args: &[&str]) -> Result<String, String> {
    let out = Command::new("adb")
        .arg("shell")
        .args(args)
        .output()
        .map_err(|e| format!("adb: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "adb shell {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Nothing may pop over the panel, so the phone is put into DND while rvnc runs. The previous
/// mode is saved first: a mode nobody set is never restored by `rvnc stop`.
fn silence_notifications() {
    match adb_shell(&["settings", "get", "global", "zen_mode"]) {
        Ok(mode) if !mode.is_empty() => {
            let _ = fs::create_dir_all(PID_DIR);
            let _ = fs::write(ZEN_FILE, &mode);
            if mode == "2" {
                println!("  {} Notifications already silenced (DND)", "✓".green());
                return;
            }
            match adb_shell(&["cmd", "notification", "set_dnd", "on"]) {
                Ok(_) => println!("  {} Notifications silenced (DND)", "✓".green()),
                Err(e) => eprintln!("  {} DND not enabled: {}", "!".yellow(), e),
            }
        }
        Ok(_) => eprintln!("  {} DND skipped: zen_mode is empty", "!".yellow()),
        Err(e) => eprintln!("  {} DND skipped: {}", "!".yellow(), e),
    }
}

fn restore_notifications() {
    let Ok(mode) = fs::read_to_string(ZEN_FILE) else { return };
    let arg = match mode.trim() {
        "0" => Some("off"),
        "1" => Some("priority"),
        _ => None, // 2 (or anything unknown) is already the safe state
    };
    if let Some(arg) = arg {
        match adb_shell(&["cmd", "notification", "set_dnd", arg]) {
            Ok(_) => println!("  {} Notifications restored (zen_mode {})", "✓".green(), mode.trim()),
            Err(e) => eprintln!("  {} Restore DND: {}", "!".yellow(), e),
        }
    }
    let _ = fs::remove_file(ZEN_FILE);
}

fn kill_pid(name: &str) -> bool {
    if let Ok(pid) = fs::read_to_string(format!("{}/{}", PID_DIR, name)) {
        let _ = Command::new("kill").arg(pid.trim()).output();
        let _ = fs::remove_file(format!("{}/{}", PID_DIR, name));
        return true;
    }
    false
}

fn is_running() -> bool {
    fs::read_to_string(format!("{}/ffmpeg", PID_DIR)).is_ok()
}

fn start(cli: Cli) {
    if wayland::is_wayland() {
        return start_wayland(cli);
    }

    if is_running() {
        println!("{} Already running. Use {} first.", "!".yellow(), "rvnc stop".cyan());
        return;
    }

    println!("{}", "rvnc".bold().magenta());
    println!("  {}",  "─────────────────────────────".dimmed());

    // 1. Check ADB
    if !phone_connected() {
        eprintln!("{} Phone not connected via USB", "✗".red());
        return;
    }

    silence_notifications();

    // Get phone resolution for proper aspect ratio
    let phone_res = get_phone_resolution();
    // Use landscape orientation for Xephyr
    let (xephyr_w, xephyr_h) = if phone_res.0 < phone_res.1 {
        (phone_res.1, phone_res.0) // flip to landscape
    } else {
        (phone_res.0, phone_res.1)
    };
    let res_str = format!("{}x{}", xephyr_w, xephyr_h);

    println!("  {} Phone {}x{}", "✓".green(), phone_res.0, phone_res.1);

    let capture_display;

    if !cli.mirror {
        // 2. Start Xephyr
        let xephyr = Command::new("Xephyr")
            .args([DISPLAY, "-screen", &res_str, "-br", "-no-host-grab", "-noreset"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn();

        match xephyr {
            Ok(c) => {
                save_pid("xephyr", c.id());
                thread::sleep(Duration::from_secs(2));
                println!("  {} Display {}", "✓".green(), DISPLAY.cyan());
            }
            Err(e) => { eprintln!("{} Xephyr: {}", "✗".red(), e); return; }
        }

        // 3. Window manager
        let wm = Command::new("openbox")
            .env("DISPLAY", DISPLAY)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn();

        if let Ok(c) = wm {
            save_pid("openbox", c.id());
            thread::sleep(Duration::from_millis(500));
            println!("  {} Window manager", "✓".green());
        }

        capture_display = DISPLAY.to_string();
    } else {
        capture_display = std::env::var("DISPLAY").unwrap_or(":0".to_string());
        println!("  {} Mirror mode {}", "✓".green(), capture_display.cyan());
    }

    // 4. ADB reverse
    let _ = Command::new("adb")
        .args(["reverse", &format!("tcp:{}", cli.port), &format!("tcp:{}", cli.port)])
        .output();
    println!("  {} ADB port {}", "✓".green(), cli.port.to_string().cyan());

    // 5. Start ffmpeg + ncat
    let ffmpeg_ncat = Command::new("bash")
        .args(["-c", &format!(
            "ffmpeg -f x11grab -framerate {fps} -video_size {res} -i {display} \
             -vaapi_device /dev/dri/renderD128 \
             -vf 'format=nv12,hwupload' \
             -c:v h264_vaapi -qp {quality} -bf 0 -g 15 \
             -flags +low_delay -fflags nobuffer -flush_packets 1 \
             -bsf:v dump_extra=freq=keyframe \
             -f h264 pipe:1 2>/dev/null | ncat -lk 0.0.0.0 {port}",
            fps = cli.fps,
            display = capture_display,
            quality = cli.quality,
            port = cli.port,
            res = res_str,
        )])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn();

    match ffmpeg_ncat {
        Ok(c) => {
            save_pid("ffmpeg", c.id());
            thread::sleep(Duration::from_secs(2));
            println!("  {} Streaming {}fps q{}", "✓".green(),
                cli.fps.to_string().cyan(), cli.quality.to_string().cyan());
        }
        Err(e) => { eprintln!("{} Stream: {}", "✗".red(), e); stop(); return; }
    }

    // Open app if requested
    if !cli.app.is_empty() {
        thread::sleep(Duration::from_millis(500));
        let mut builder = Command::new(&cli.app[0]);
        builder.args(&cli.app[1..]).env("DISPLAY", DISPLAY)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if is_chromium_based(&cli.app[0]) {
            builder.arg("--user-data-dir=/tmp/rvnc-browser-profile");
        }
        let _ = builder.spawn();
        println!("  {} {}", "✓".green(), cli.app[0].cyan());
    }

    // Save config
    let config = format!("{}|{}|{}|{}", cli.port, cli.fps, cli.quality, capture_display);
    let _ = fs::write(format!("{}/config", PID_DIR), config);

    println!("  {}",  "─────────────────────────────".dimmed());
    println!("  {} Ready! Open rVNC on phone", "☎".magenta());

    if !cli.mirror {
        println!();
        println!("  {} Open apps:", "i".blue());
        println!("    {} firefox", "rvnc open".cyan());
        println!("    {} mpv video.mp4", "rvnc open".cyan());
    }
}

fn phone_connected() -> bool {
    Command::new("adb")
        .args(["devices"])
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.lines().filter(|l| l.contains("device") && !l.contains("List")).count() > 0
        })
        .unwrap_or(false)
}

/// Wayland/Hyprland: the phone gets a real headless output instead of a nested Xephyr window.
fn start_wayland(cli: Cli) {
    if wayland::stream_running() {
        println!("{} Already running. Use {} first.", "!".yellow(), "rvnc stop".cyan());
        return;
    }

    println!("{}", "rvnc (wayland)".bold().magenta());
    println!("  {}", "─────────────────────────────".dimmed());

    if !phone_connected() {
        eprintln!("{} Phone not connected via USB", "✗".red());
        return;
    }

    silence_notifications();

    // The phone app is locked to landscape, so the output is landscape too.
    let phone_res = get_phone_resolution();
    let (w, h) = if phone_res.0 < phone_res.1 { (phone_res.1, phone_res.0) } else { phone_res };
    // Physical parity, not a guess: a 29" 2560x1440 monitor at scale 1.333 is ~135 logical
    // px per inch, the 6.6" 1080x2400 panel is ~399 ppi, so 399/135 ~= 3 (= density/160).
    // At scale 3 the logical canvas is 800x360 — that is genuinely all a 6.6" screen holds,
    // so the panel has to be built for 800x360 instead of being a desktop TUI.
    let scale = cli.scale.unwrap_or_else(|| {
        ((get_phone_density() as f32 / 160.0).clamp(1.0, 3.0) * 100.0).round() / 100.0
    });
    println!("  {} Phone {}x{} @{}dpi", "✓".green(), phone_res.0, phone_res.1, get_phone_density());
    println!("  {} Output {} {}x{} scale {} (lives while the phone is connected)",
        "✓".green(), wayland::OUTPUT.cyan(), w, h, scale);

    let _ = Command::new("adb")
        .args(["reverse", &format!("tcp:{}", cli.port), &format!("tcp:{}", cli.port)])
        .output();
    println!("  {} ADB port {}", "✓".green(), cli.port.to_string().cyan());

    match wayland::start_stream(cli.port, cli.fps, cli.quality, w, h, scale) {
        Ok(_) => println!("  {} Streaming {}fps q{}", "✓".green(),
            wayland::capture_fps(cli.fps).to_string().cyan(), cli.quality.to_string().cyan()),
        Err(e) => {
            eprintln!("{} Stream: {}", "✗".red(), e);
            stop();
            return;
        }
    }

    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(format!("{}/config", PID_DIR),
        format!("{}|{}|{}|{}", cli.port, wayland::capture_fps(cli.fps), cli.quality, wayland::OUTPUT));

    println!("  {}", "─────────────────────────────".dimmed());
    println!("  {} Ready! Open rVNC on phone", "☎".magenta());
    println!();
    println!("  {} rvnc-panel opens on every phone connection; other apps:", "i".blue());
    println!("    {} firefox", "rvnc open".cyan());
}

/// `rvnc-panel` is shipped next to `rvnc`; btop and the terminal wrapper are gone on purpose.
fn panel_binary() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current executable: {}", e))?;
    let dir = exe.parent().ok_or_else(|| "current executable has no directory".to_string())?;
    let path = dir.join("rvnc-panel");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{} not found — build it with {}",
            path.display(),
            "cargo build --release".cyan()
        ))
    }
}

fn panel(cmd: Vec<String>) {
    let auto = cmd.is_empty();
    let cmd = if auto {
        match panel_binary() {
            Ok(p) => vec![p.to_string_lossy().to_string()],
            Err(e) => {
                eprintln!("{} {}", "✗".red(), e);
                return;
            }
        }
    } else {
        cmd
    };

    if wayland::output_exists() {
        // The move + fullscreen dispatchers focus the phone output and park the pointer there,
        // which wf-recorder would capture as a cursor over the panel. Both are put back after.
        let previous_focus = wayland::focused_monitor();
        let previous_cursor = wayland::cursor_pos();
        match wayland::launch_panel(&cmd, !auto) {
            Ok(pid) => {
                if auto {
                    match wayland::fullscreen_pid(pid) {
                        Ok(()) => println!("{} {} fullscreen on phone", "✓".green(), cmd[0].cyan()),
                        Err(e) => eprintln!("{} moved, not fullscreen: {}", "!".yellow(), e),
                    }
                    if let Some(name) = previous_focus {
                        if let Err(e) = wayland::focus_monitor(&name) {
                            eprintln!("{} focus {} not restored: {}", "!".yellow(), name, e);
                        }
                    }
                    if let Some((x, y)) = previous_cursor {
                        if let Err(e) = wayland::cursor_move(x, y) {
                            eprintln!("{} cursor not restored: {}", "!".yellow(), e);
                        }
                    }
                } else {
                    println!("{} {} on phone", "✓".green(), cmd[0].cyan());
                }
            }
            Err(e) => eprintln!("{} {}", "✗".red(), e),
        }
    } else if is_running() {
        let child = Command::new(&cmd[0]).args(&cmd[1..]).env("DISPLAY", DISPLAY)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        match child {
            Ok(_) => println!("{} {} on phone", "✓".green(), cmd[0].cyan()),
            Err(e) => eprintln!("{} {}: {}", "✗".red(), cmd[0], e),
        }
    } else {
        eprintln!("{} Not running. Start with {}", "!".yellow(), "rvnc".cyan());
    }
}

fn stop() {
    // Must run before wayland::stop wipes /tmp/rvnc, which holds the saved mode.
    restore_notifications();

    if wayland::stream_running() || wayland::output_exists() {
        println!("{} Stopping rvnc (wayland)...", "→".blue());
        if let Ok(config) = fs::read_to_string(format!("{}/config", PID_DIR)) {
            if let Some(port) = config.split('|').next() {
                let _ = Command::new("adb")
                    .args(["reverse", "--remove", &format!("tcp:{}", port)])
                    .output();
            }
        }
        wayland::stop();
        wayland::remove_output();
        println!("{} Stopped", "✓".green());
        return;
    }

    println!("{} Stopping rvnc...", "→".blue());

    // Kill in reverse order
    if kill_pid("ffmpeg") {
        // Also kill child ffmpeg/ncat
        let _ = Command::new("pkill").args(["-f", "ncat.*8800"]).output();
        let _ = Command::new("pkill").args(["-f", "ffmpeg.*x11grab"]).output();
        println!("  {} Stream", "✓".green());
    }
    kill_pid("panel");
    kill_pid("openbox");
    if kill_pid("xephyr") {
        let _ = Command::new("pkill").args(["-f", "Xephyr.*:9"]).output();
        println!("  {} Display", "✓".green());
    }

    let _ = Command::new("adb").args(["reverse", "--remove-all"]).output();
    let _ = fs::remove_dir_all(PID_DIR);

    println!("{} Stopped", "✓".green());
}

fn status() {
    println!("{}", "rvnc".bold().magenta());

    if let Ok(config) = fs::read_to_string(format!("{}/config", PID_DIR)) {
        let parts: Vec<&str> = config.split('|').collect();
        let running = is_running();

        println!("  Status:  {}", if running { "active".green().bold() } else { "dead".red().bold() });
        if parts.len() >= 4 {
            println!("  Port:    {}", parts[0].cyan());
            println!("  FPS:     {}", parts[1].cyan());
            println!("  Quality: {}", parts[2].cyan());
            println!("  Display: {}", parts[3].cyan());
        }
        println!("  Phone:   {}:{}", "127.0.0.1".cyan(),
            parts.first().unwrap_or(&"8800").cyan());
        if wayland::output_exists() {
            println!("  Output:  {} (headless)", wayland::OUTPUT.cyan());
        }
    } else {
        println!("  Status:  {}", "inactive".dimmed());
    }
}

fn get_phone_density() -> u32 {
    Command::new("adb")
        .args(["shell", "wm", "density"])
        .output()
        .ok()
        .and_then(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            // "Physical density: 480"
            out.split(':').nth(1).and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or(160)
}

fn get_phone_resolution() -> (u32, u32) {
    Command::new("adb")
        .args(["shell", "wm", "size"])
        .output()
        .ok()
        .and_then(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            // "Physical size: 1080x2400"
            out.split(':').nth(1)
                .and_then(|s| {
                    let parts: Vec<&str> = s.trim().split('x').collect();
                    if parts.len() == 2 {
                        let w: u32 = parts[0].parse().ok()?;
                        let h: u32 = parts[1].parse().ok()?;
                        Some((w, h))
                    } else { None }
                })
        })
        .unwrap_or((1920, 1080))
}

fn is_chromium_based(name: &str) -> bool {
    matches!(name, "brave" | "brave-browser" | "chromium" | "chromium-browser" | "google-chrome" | "chrome")
}

fn open_app(cmd: &[String]) {
    if cmd.is_empty() {
        eprintln!("{} Usage: rvnc open firefox", "✗".red());
        return;
    }
    if wayland::output_exists() {
        match wayland::launch_panel(cmd, true) {
            Ok(_) => println!("{} {} on phone", "✓".green(), cmd[0].cyan()),
            Err(e) => eprintln!("{} {}", "✗".red(), e),
        }
        return;
    }
    if !is_running() {
        println!("{} Not running. Start with {}", "!".yellow(), "rvnc".cyan());
        return;
    }

    let mut builder = Command::new(&cmd[0]);
    builder.args(&cmd[1..]).env("DISPLAY", DISPLAY)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    if is_chromium_based(&cmd[0]) {
        builder.arg("--user-data-dir=/tmp/rvnc-browser-profile");
    }

    match builder.spawn() {
        Ok(_) => println!("{} {} on display {}", "✓".green(), cmd[0].cyan(), DISPLAY.cyan()),
        Err(e) => eprintln!("{} {}: {}", "✗".red(), cmd[0], e),
    }
}
