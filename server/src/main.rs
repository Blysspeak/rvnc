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
    /// Run a panel app on the phone display (default: btop)
    Panel {
        /// Command to run
        cmd: Vec<String>,
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
        None => start(cli),
    }
}

fn save_pid(name: &str, pid: u32) {
    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(format!("{}/{}", PID_DIR, name), pid.to_string());
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

    // The phone app is locked to landscape, so the output is landscape too.
    let phone_res = get_phone_resolution();
    let (w, h) = if phone_res.0 < phone_res.1 { (phone_res.1, phone_res.0) } else { phone_res };
    println!("  {} Phone {}x{}", "✓".green(), phone_res.0, phone_res.1);

    match wayland::ensure_output(w, h) {
        Ok(_) => println!("  {} Output {} {}x{}", "✓".green(), wayland::OUTPUT.cyan(), w, h),
        Err(e) => { eprintln!("{} Output: {}", "✗".red(), e); return; }
    }

    let _ = Command::new("adb")
        .args(["reverse", &format!("tcp:{}", cli.port), &format!("tcp:{}", cli.port)])
        .output();
    println!("  {} ADB port {}", "✓".green(), cli.port.to_string().cyan());

    match wayland::start_stream(cli.port, cli.fps, cli.quality) {
        Ok(_) => println!("  {} Streaming {}fps q{}", "✓".green(),
            cli.fps.to_string().cyan(), cli.quality.to_string().cyan()),
        Err(e) => {
            eprintln!("{} Stream: {}", "✗".red(), e);
            wayland::remove_output();
            return;
        }
    }

    let _ = fs::create_dir_all(PID_DIR);
    let _ = fs::write(format!("{}/config", PID_DIR),
        format!("{}|{}|{}|{}", cli.port, cli.fps, cli.quality, wayland::OUTPUT));

    println!("  {}", "─────────────────────────────".dimmed());
    println!("  {} Ready! Open rVNC on phone", "☎".magenta());
    println!();
    println!("  {} Fill the screen:", "i".blue());
    println!("    {} btop", "rvnc panel".cyan());
    println!("    {} firefox", "rvnc open".cyan());
}

fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(prog).is_file()))
        .unwrap_or(false)
}

fn default_panel() -> Vec<String> {
    if which("btop") && which("foot") {
        vec!["foot".into(), "-e".into(), "btop".into()]
    } else {
        vec!["foot".into()]
    }
}

fn panel(cmd: Vec<String>) {
    let cmd = if cmd.is_empty() { default_panel() } else { cmd };

    if wayland::output_exists() {
        match wayland::launch_panel(&cmd) {
            Ok(_) => println!("{} {} on phone", "✓".green(), cmd[0].cyan()),
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
        match wayland::launch_panel(cmd) {
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
