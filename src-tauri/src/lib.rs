//! Lucid — a macOS menu-bar app that keeps your Mac awake via a managed
//! `caffeinate` child process. When active, the tray icon glows amber and
//! gently pulses; with a timer it shows a live MM:SS countdown beside the icon.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    image::Image,
    menu::{CheckMenuItem, MenuBuilder, MenuItem, SubmenuBuilder},
    tray::TrayIconBuilder,
    Manager,
};

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
    toggle_item: CheckMenuItem<tauri::Wry>,
    status_item: MenuItem<tauri::Wry>,
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

/// Start keeping the Mac awake. `total_secs = None` means indefinite.
fn start_session(app: &tauri::AppHandle, ctx: &Ctx, total_secs: Option<u64>) {
    stop_session(ctx);

    let mut cmd = Command::new("caffeinate");
    cmd.arg("-d").arg("-i"); // prevent display + idle sleep
    if let Some(s) = total_secs {
        cmd.arg("-t").arg(s.to_string());
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = ctx.status_item.set_text(&format!("Error: {e}"));
            return;
        }
    };
    *ctx.child.lock().unwrap() = Some(child);

    // Immediate feedback (this runs on the main thread, from the menu handler).
    let _ = ctx.toggle_item.set_checked(true);
    let _ = ctx.status_item.set_text(match total_secs {
        None => "Awake — until turned off".to_string(),
        Some(s) => format!("Awake — {} left", fmt_remaining(s)),
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

            // Timer elapsed -> switch off on the main thread.
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

            // Only push new text when the displayed second actually changes.
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let toggle =
                CheckMenuItem::with_id(app, "toggle", "Keep awake", true, false, None::<&str>)?;
            let status = MenuItem::with_id(app, "status", "Sleep allowed", false, None::<&str>)?;

            let timer = SubmenuBuilder::new(app, "Keep awake for…")
                .text("dur:1800", "30 minutes")
                .text("dur:3600", "1 hour")
                .text("dur:7200", "2 hours")
                .text("dur:18000", "5 hours")
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&status)
                .separator()
                .item(&toggle)
                .item(&timer)
                .separator()
                .text("quit", "Quit Lucid")
                .build()?;

            app.manage(Ctx {
                child: Mutex::new(None),
                generation: AtomicU64::new(0),
                toggle_item: toggle.clone(),
                status_item: status.clone(),
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
                        "quit" => {
                            stop_session(ctx);
                            app.exit(0);
                        }
                        id => {
                            if let Some(secs) =
                                id.strip_prefix("dur:").and_then(|s| s.parse::<u64>().ok())
                            {
                                start_session(app, ctx, Some(secs));
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
