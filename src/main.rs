#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bezier;
mod canvas;
mod canvas_sort;
pub mod command;
mod config;
mod export;
mod flatten;
mod image_ops;
mod lama_cleaner;
mod loader;
mod native_inpaint;
mod pathops;
mod portal_capture;
mod recents;
pub mod region_select;
pub mod render_xform;
mod scene;
mod session;
mod shortcuts;
pub mod smoothing;
mod stamps;
mod state;
mod svg_emit;
mod texture_upload;
mod theme;
mod toolbar;
mod tools;
pub mod transform;
pub mod types;
mod vectorize;
mod vectorize_worker;

#[cfg(feature = "agent-server")]
mod agent;

#[cfg(feature = "agent-server")]
mod text_render;

#[cfg(feature = "local-ai")]
mod local_ai;

use app::RedLineApp;
use state::SnapMode;

fn parse_capture_arg() -> Option<SnapMode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let value = if let Some(v) = a.strip_prefix("--capture=") {
            Some(v.to_string())
        } else if a == "--capture" {
            i += 1;
            args.get(i).cloned()
        } else {
            None
        };
        if let Some(v) = value {
            return match v.as_str() {
                "region" => Some(SnapMode::Region),
                "full" => Some(SnapMode::Full),
                other => {
                    eprintln!("unknown --capture value: {other} (expected region|full)");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// First non-flag argument, treated as a file to open at startup —
/// `redline shot.png` is the shortest path from a terminal to an annotation.
fn parse_file_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--capture" {
            let _ = args.next(); // its value
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        let path = std::path::PathBuf::from(&a);
        if path.exists() {
            return Some(path);
        }
        eprintln!("redline: no such file: {a}");
    }
    None
}

/// Minimal stderr logger for the windowing/GL stack. eframe and winit report
/// real failures (a viewport that couldn't be created, GL context problems)
/// through `log`, which is silently dropped without a logger installed — set
/// `REDLINE_LOG=1` to see them.
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }
    fn flush(&self) {}
}

fn init_logging() {
    if std::env::var_os("REDLINE_LOG").is_some() {
        let _ = log::set_boxed_logger(Box::new(StderrLogger));
        log::set_max_level(log::LevelFilter::Warn);
    }
}

fn main() -> eframe::Result<()> {
    init_logging();
    // Lightweight flags that print and exit without spinning up the GUI — so e.g.
    // `redline --version` is safe to call from scripts (install.sh prints it).
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--version" | "-V" => {
                println!("RedLine {}", env!("REDLINE_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "RedLine {}\n\nUsage: redline [FILE] [--capture region|full] [--version]",
                    env!("REDLINE_VERSION")
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let startup_capture = parse_capture_arg();
    let startup_file = parse_file_arg();

    // Initialize GTK before eframe so the embedded Lama-Cleaner webview can
    // create WebKit child windows inside the main eframe window.  GTK and
    // winit (used by eframe) each open their own X11/Wayland connection; they
    // coexist safely on the same thread as long as we pump GTK events manually
    // from within eframe's update() loop (see app.rs).
    #[cfg(feature = "lama-webview")]
    {
        if let Err(e) = gtk::init() {
            eprintln!("lama-webview: gtk::init failed ({e:?}); embedded webview disabled");
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title(concat!("RedLine ", env!("REDLINE_VERSION"))),
        ..Default::default()
    };
    eframe::run_native(
        "RedLine",
        options,
        Box::new(move |cc| {
            let settings = crate::config::load_settings();
            let mut app = RedLineApp::with_settings(settings);
            app.state.startup_capture = startup_capture.clone();
            app.state.startup_file = startup_file.clone();
            app.state.recent_captures = crate::state::load_recent_captures();
            let last_dirs = crate::state::load_last_dirs();
            app.state.last_open_dir = last_dirs.open;
            app.state.last_save_dir = last_dirs.save;
            // Restore the previous session's tabs (opt-out via Settings).
            if app.state.settings.restore_session {
                if let Some(session) = crate::session::load_session() {
                    crate::session::restore_into(&mut app.state, &cc.egui_ctx, session);
                }
            }
            Ok(Box::new(app))
        }),
    )
}
