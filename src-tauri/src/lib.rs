//! Lucid — a macOS menu-bar app that keeps your Mac awake via a managed
//! `caffeinate` child process. When active, the tray icon glows amber and
//! gently pulses; with a timer it shows a live MM:SS countdown beside the icon.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    image::Image,
    menu::{CheckMenuItem, MenuBuilder, MenuItem, SubmenuBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const ICON_OFF: &[u8] = include_bytes!("../icons/tray-off.png");
const PULSE: [&[u8]; 5] = [
    include_bytes!("../icons/tray-on-0.png"),
    include_bytes!("../icons/tray-on-1.png"),
    include_bytes!("../icons/tray-on-2.png"),
    include_bytes!("../icons/tray-on-3.png"),
    include_bytes!("../icons/tray-on-4.png"),
];
/// Ping-pong over the frames for a smooth "breathing" pulse.
const SEQ: [usize; 8] = [0, 1, 2, 3, 4, 3, 2, 1];
const TICK_MS: u64 = 150;
const TRAY_ID: &str = "lucid-tray";

struct Ctx {
    child: Mutex<Option<Child>>,
    /// Bumped on every state change; lets stale animation threads bail out.
    generation: AtomicU64,
    /// Whether to also keep the display on (`-d`) vs. just the system (`-i`).
    keep_display: AtomicBool,
    /// The current session's total (None = indefinite) and start, so we can
    /// restart with the correct remaining time when settings change.
    current_total: Mutex<Option<u64>>,
    current_start: Mutex<Option<Instant>>,
    toggle_item: CheckMenuItem<tauri::Wry>,
    status_item: MenuItem<tauri::Wry>,
    display_item: CheckMenuItem<tauri::Wry>,
    autostart_item: CheckMenuItem<tauri::Wry>,
}

fn fmt_remaining(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Kill the current caffeinate process (if any) and invalidate pending timers.
fn stop_session(ctx: &Ctx) {
    ctx.generation.fetch_add(1, Ordering::SeqCst);
    *ctx.current_total.lock().unwrap() = None;
    *ctx.current_start.lock().unwrap() = None;
    if let Some(mut child) = ctx.child.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Reset the menu bar to the idle (sleep-allowed) look.
fn set_off(app: &tauri::AppHandle, ctx: &Ctx) {
    let _ = ctx.toggle_item.set_checked(false);
    let _ = ctx.status_item.set_text("Sleep allowed");
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(None::<&str>);
        if let Ok(img) = Image::from_bytes(ICON_OFF) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_icon_as_template(true); // monochrome ring adapts to the bar
        let _ = tray.set_tooltip(Some("Lucid — sleep allowed"));
    }
}

/// Spawn a `caffeinate` child honoring the "keep display on" setting.
/// `total_secs = Some(n)` auto-expires after n seconds; `None` runs until killed.
fn spawn_caffeinate(ctx: &Ctx, total_secs: Option<u64>) -> std::io::Result<Child> {
    let mut cmd = Command::new("caffeinate");
    cmd.arg("-i"); // prevent system idle sleep
    if ctx.keep_display.load(Ordering::SeqCst) {
        cmd.arg("-d"); // also keep the display on
    }
    if let Some(s) = total_secs {
        cmd.arg("-t").arg(s.to_string());
    }
    cmd.spawn()
}

/// Start keeping the Mac awake. `total_secs = None` means indefinite.
fn start_session(app: &tauri::AppHandle, ctx: &Ctx, total_secs: Option<u64>) {
    stop_session(ctx);
    *ctx.current_total.lock().unwrap() = total_secs;
    *ctx.current_start.lock().unwrap() = Some(Instant::now());

    let child = match spawn_caffeinate(ctx, total_secs) {
        Ok(c) => c,
        Err(e) => {
            let _ = ctx.status_item.set_text(&format!("Error: {e}"));
            return;
        }
    };
    *ctx.child.lock().unwrap() = Some(child);

    let display_note = if ctx.keep_display.load(Ordering::SeqCst) { "" } else { " (display may sleep)" };
    let _ = ctx.toggle_item.set_checked(true);
    let _ = ctx.status_item.set_text(&match total_secs {
        None => format!("Awake — until turned off{display_note}"),
        Some(s) => format!("Awake — {} left{display_note}", fmt_remaining(s)),
    });
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(img) = Image::from_bytes(PULSE[0]) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_icon_as_template(false); // amber stays colored
        let _ = tray.set_tooltip(Some("Lucid — keeping your Mac awake"));
        match total_secs {
            Some(s) => {
                let _ = tray.set_title(Some(fmt_remaining(s).as_str()));
            }
            None => {
                let _ = tray.set_title(None::<&str>);
            }
        }
    }

    // Animation/countdown loop, valid only for this generation.
    let gen = ctx.generation.load(Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut step: usize = 0;
        let mut last_shown: Option<u64> = None;
        loop {
            {
                let ctx = app.state::<Ctx>();
                if ctx.generation.load(Ordering::SeqCst) != gen {
                    return; // superseded by a newer session / turned off
                }
            }
            let elapsed = start.elapsed().as_secs();

            if let Some(total) = total_secs {
                if elapsed >= total {
                    let app2 = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        let ctx = app2.state::<Ctx>();
                        if ctx.generation.load(Ordering::SeqCst) == gen {
                            if let Some(mut c) = ctx.child.lock().unwrap().take() {
                                let _ = c.wait();
                            }
                            set_off(&app2, ctx.inner());
                        }
                    });
                    return;
                }
            }

            let frame = PULSE[SEQ[step % SEQ.len()]];
            step += 1;

            let text: Option<String> = total_secs.and_then(|total| {
                let rem = total.saturating_sub(elapsed);
                if last_shown != Some(rem) {
                    last_shown = Some(rem);
                    Some(fmt_remaining(rem))
                } else {
                    None
                }
            });

            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                let ctx = app2.state::<Ctx>();
                if ctx.generation.load(Ordering::SeqCst) != gen {
                    return;
                }
                if let Some(tray) = app2.tray_by_id(TRAY_ID) {
                    if let Ok(img) = Image::from_bytes(frame) {
                        let _ = tray.set_icon(Some(img));
                    }
                    let _ = tray.set_icon_as_template(false);
                    if let Some(ref t) = text {
                        let _ = tray.set_title(Some(t.as_str()));
                    }
                }
                if let Some(ref t) = text {
                    let _ = ctx.status_item.set_text(&format!("Awake — {t} left"));
                }
            });

            std::thread::sleep(Duration::from_millis(TICK_MS));
        }
    });
}

/// The three Pomodoro phases.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

fn phase_emoji(p: Phase) -> &'static str {
    match p {
        Phase::Focus => "🍅",
        _ => "☕",
    }
}

/// Given the phase that just finished, compute the next phase and focus-round.
fn next_phase(phase: Phase, round: u32, rounds: u32) -> (Phase, u32) {
    match phase {
        Phase::Focus if round >= rounds => (Phase::LongBreak, round),
        Phase::Focus => (Phase::ShortBreak, round),
        Phase::ShortBreak => (Phase::Focus, round + 1),
        Phase::LongBreak => (Phase::Focus, 1),
    }
}

/// Post a macOS notification + chime — best-effort, never blocks the loop.
fn notify(title: &str, body: &str) {
    let script = format!("display notification {body:?} with title {title:?}");
    std::thread::spawn(move || {
        let _ = Command::new("osascript").arg("-e").arg(&script).status();
    });
    std::thread::spawn(|| {
        let _ = Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .status();
    });
}

/// Start a Pomodoro cycle: keep the Mac awake throughout, count each phase down
/// in the menu bar, and notify (with a chime) on every focus⇄break transition.
fn start_pomodoro(
    app: &tauri::AppHandle,
    ctx: &Ctx,
    focus: u64,
    short_break: u64,
    long_break: u64,
    rounds: u32,
) {
    stop_session(ctx);

    let child = match spawn_caffeinate(ctx, None) {
        Ok(c) => c,
        Err(e) => {
            let _ = ctx.status_item.set_text(&format!("Error: {e}"));
            return;
        }
    };
    *ctx.child.lock().unwrap() = Some(child);

    let _ = ctx.toggle_item.set_checked(false);
    let _ = ctx.status_item.set_text(&format!(
        "Focus — round 1/{rounds} · {} left",
        fmt_remaining(focus)
    ));
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(img) = Image::from_bytes(PULSE[0]) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_icon_as_template(false);
        let _ = tray.set_tooltip(Some("Lucid — Pomodoro"));
        let _ = tray.set_title(Some(format!("🍅 {}", fmt_remaining(focus)).as_str()));
    }

    let gen = ctx.generation.load(Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        let mut phase = Phase::Focus;
        let mut round: u32 = 1;
        let mut phase_total = focus;
        let mut phase_start = Instant::now();
        let mut step: usize = 0;
        let mut last_shown: Option<u64> = None;
        loop {
            {
                let ctx = app.state::<Ctx>();
                if ctx.generation.load(Ordering::SeqCst) != gen {
                    return; // stopped or superseded
                }
            }

            // Phase complete -> advance and announce.
            if phase_start.elapsed().as_secs() >= phase_total {
                let (next, next_round) = next_phase(phase, round, rounds);
                let body = match next {
                    Phase::ShortBreak => {
                        format!("Focus done — take a {}-minute break.", short_break / 60)
                    }
                    Phase::LongBreak => {
                        format!("Great work! Take a longer {}-minute break.", long_break / 60)
                    }
                    Phase::Focus => {
                        format!("Break's over — back to focus (round {next_round} of {rounds}).")
                    }
                };
                notify("Lucid 🍅", &body);
                phase = next;
                round = next_round;
                phase_total = match phase {
                    Phase::Focus => focus,
                    Phase::ShortBreak => short_break,
                    Phase::LongBreak => long_break,
                };
                phase_start = Instant::now();
                last_shown = None;
            }

            let rem = phase_total.saturating_sub(phase_start.elapsed().as_secs());
            let frame = PULSE[SEQ[step % SEQ.len()]];
            step += 1;
            let push_text = last_shown != Some(rem);
            if push_text {
                last_shown = Some(rem);
            }
            let title_txt = format!("{} {}", phase_emoji(phase), fmt_remaining(rem));
            let status_txt = match phase {
                Phase::Focus => {
                    format!("Focus — round {round}/{rounds} · {} left", fmt_remaining(rem))
                }
                Phase::ShortBreak => format!("Break — {} left", fmt_remaining(rem)),
                Phase::LongBreak => format!("Long break — {} left", fmt_remaining(rem)),
            };

            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                let ctx = app2.state::<Ctx>();
                if ctx.generation.load(Ordering::SeqCst) != gen {
                    return;
                }
                if let Some(tray) = app2.tray_by_id(TRAY_ID) {
                    if let Ok(img) = Image::from_bytes(frame) {
                        let _ = tray.set_icon(Some(img));
                    }
                    let _ = tray.set_icon_as_template(false);
                    if push_text {
                        let _ = tray.set_title(Some(title_txt.as_str()));
                    }
                }
                if push_text {
                    let _ = ctx.status_item.set_text(&status_txt);
                }
            });

            std::thread::sleep(Duration::from_millis(TICK_MS));
        }
    });
}

/// Toggle "keep display on"; re-applies to the running session with its remaining time.
fn toggle_display(app: &tauri::AppHandle, ctx: &Ctx) {
    let new = !ctx.keep_display.load(Ordering::SeqCst);
    ctx.keep_display.store(new, Ordering::SeqCst);
    let _ = ctx.display_item.set_checked(new);
    if ctx.child.lock().unwrap().is_some() {
        let total = *ctx.current_total.lock().unwrap();
        let start = *ctx.current_start.lock().unwrap();
        let remaining = match (total, start) {
            (Some(t), Some(st)) => Some(t.saturating_sub(st.elapsed().as_secs())),
            _ => None,
        };
        start_session(app, ctx, remaining);
    }
}

/// Toggle launch-at-login via the autostart plugin.
fn toggle_autostart(app: &tauri::AppHandle, ctx: &Ctx) {
    let mgr = app.autolaunch();
    if mgr.is_enabled().unwrap_or(false) {
        let _ = mgr.disable();
    } else {
        let _ = mgr.enable();
    }
    let _ = ctx.autostart_item.set_checked(mgr.is_enabled().unwrap_or(false));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let toggle =
                CheckMenuItem::with_id(app, "toggle", "Keep awake", true, false, None::<&str>)?;
            let status = MenuItem::with_id(app, "status", "Sleep allowed", false, None::<&str>)?;

            let timer = SubmenuBuilder::new(app, "Keep awake for…")
                .text("dur:900", "15 minutes")
                .text("dur:1800", "30 minutes")
                .text("dur:3600", "1 hour")
                .text("dur:7200", "2 hours")
                .text("dur:14400", "4 hours")
                .text("dur:28800", "8 hours")
                .build()?;

            // focus : short break : long break : rounds-before-long-break (seconds)
            let pomodoro = SubmenuBuilder::new(app, "Pomodoro")
                .text("pomo:1500:300:900:4", "Classic — 25 / 5")
                .text("pomo:3000:600:1800:4", "Deep work — 50 / 10")
                .separator()
                .text("pomo:stop", "Stop Pomodoro")
                .build()?;

            let display =
                CheckMenuItem::with_id(app, "display", "Keep display on", true, true, None::<&str>)?;
            let auto_enabled = app.autolaunch().is_enabled().unwrap_or(false);
            let autostart = CheckMenuItem::with_id(
                app,
                "autostart",
                "Launch at login",
                true,
                auto_enabled,
                None::<&str>,
            )?;

            let menu = MenuBuilder::new(app)
                .item(&status)
                .separator()
                .item(&toggle)
                .item(&timer)
                .item(&pomodoro)
                .separator()
                .item(&display)
                .item(&autostart)
                .separator()
                .text("quit", "Quit Lucid")
                .build()?;

            app.manage(Ctx {
                child: Mutex::new(None),
                generation: AtomicU64::new(0),
                keep_display: AtomicBool::new(true),
                current_total: Mutex::new(None),
                current_start: Mutex::new(None),
                toggle_item: toggle.clone(),
                status_item: status.clone(),
                display_item: display.clone(),
                autostart_item: autostart.clone(),
            });

            let off_icon = Image::from_bytes(ICON_OFF)?;
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(off_icon)
                .icon_as_template(true)
                .tooltip("Lucid — sleep allowed")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    let ctx = app.state::<Ctx>();
                    let ctx = ctx.inner();
                    match event.id().as_ref() {
                        "toggle" => {
                            let on = ctx.child.lock().unwrap().is_some();
                            if on {
                                stop_session(ctx);
                                set_off(app, ctx);
                            } else {
                                start_session(app, ctx, None);
                            }
                        }
                        "display" => toggle_display(app, ctx),
                        "autostart" => toggle_autostart(app, ctx),
                        "quit" => {
                            stop_session(ctx);
                            app.exit(0);
                        }
                        id => {
                            if let Some(secs) =
                                id.strip_prefix("dur:").and_then(|s| s.parse::<u64>().ok())
                            {
                                start_session(app, ctx, Some(secs));
                            } else if let Some(rest) = id.strip_prefix("pomo:") {
                                if rest == "stop" {
                                    stop_session(ctx);
                                    set_off(app, ctx);
                                } else {
                                    let p: Vec<&str> = rest.split(':').collect();
                                    if let [f, s, l, r] = p[..] {
                                        if let (Ok(f), Ok(s), Ok(l), Ok(r)) = (
                                            f.parse::<u64>(),
                                            s.parse::<u64>(),
                                            l.parse::<u64>(),
                                            r.parse::<u32>(),
                                        ) {
                                            start_pomodoro(app, ctx, f, s, l, r);
                                        }
                                    }
                                }
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
