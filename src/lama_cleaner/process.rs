use std::io::BufRead;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use super::state::{InstanceStatus, LamaMsg, SetupStatus};

pub const PORT_SEARCH_START: u16 = 9200;

// ── Paths ─────────────────────────────────────────────────────────────────────

/// Per-user data root. Never falls back to a shared, predictable location
/// like `/tmp`, where another local user could pre-plant a venv or models.
fn data_root() -> PathBuf {
    dirs_next::data_dir()
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("redline")
}

pub fn venv_dir() -> PathBuf {
    data_root().join("lama-venv")
}

pub fn lama_bin() -> PathBuf {
    venv_dir().join("bin").join("lama-cleaner")
}

pub fn model_dir() -> PathBuf {
    data_root().join("lama-models")
}

// ── Port discovery ────────────────────────────────────────────────────────────

pub fn find_free_port() -> Option<u16> {
    for port in PORT_SEARCH_START..PORT_SEARCH_START + 300 {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

// ── Setup ─────────────────────────────────────────────────────────────────────

/// Check whether the venv + lama-cleaner binary are present; non-blocking.
pub fn check_setup(tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        let _ = tx.send(LamaMsg::SetupStatus(SetupStatus::Checking));
        let status = if lama_bin().exists() {
            SetupStatus::Ready
        } else {
            SetupStatus::NeedsInstall
        };
        let _ = tx.send(LamaMsg::SetupStatus(status));
    });
}

/// Create venv + pip-install lama-cleaner, streaming output as SetupLog messages.
pub fn install(tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        let send_s = |s: SetupStatus| {
            let _ = tx.send(LamaMsg::SetupStatus(s));
        };
        let send_l = |l: &str| {
            let _ = tx.send(LamaMsg::SetupLog(l.to_owned()));
        };

        send_s(SetupStatus::Installing);

        let venv = venv_dir();
        if !venv.exists() {
            send_l("Creating Python virtual environment…");
            match Command::new("python3")
                .args(["-m", "venv", venv.to_str().unwrap_or("")])
                .output()
            {
                Ok(o) if o.status.success() => send_l("Virtual environment created."),
                Ok(o) => {
                    let e = String::from_utf8_lossy(&o.stderr).trim().to_owned();
                    send_l(&format!("Error: {e}"));
                    send_s(SetupStatus::Error(e));
                    return;
                }
                Err(e) => {
                    send_s(SetupStatus::Error(e.to_string()));
                    return;
                }
            }
        }

        send_l("Installing lama-cleaner — this downloads ~1 GB the first time, please wait…");

        let pip = venv.join("bin").join("pip");
        // huggingface_hub>=0.17 removed `cached_download`; diffusers==0.16.1
        // (pinned by lama-cleaner) still imports it, so we cap the version here.
        let mut child = match Command::new(&pip)
            .args(["install", "lama-cleaner==1.2.5", "huggingface_hub==0.16.4"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                send_s(SetupStatus::Error(e.to_string()));
                return;
            }
        };

        // Pipe stdout + stderr as log messages (pip uses stderr for progress).
        let pip_streams: [Option<Box<dyn std::io::Read + Send>>; 2] = [
            child.stdout.take().map(|s| Box::new(s) as _),
            child.stderr.take().map(|s| Box::new(s) as _),
        ];
        for stream in pip_streams {
            if let Some(r) = stream {
                let tx2 = tx.clone();
                std::thread::spawn(move || {
                    for line in std::io::BufReader::new(r).lines().map_while(Result::ok) {
                        let _ = tx2.send(LamaMsg::SetupLog(line));
                    }
                });
            }
        }

        match child.wait() {
            Ok(s) if s.success() => {
                send_l("Installation complete.");
                send_s(SetupStatus::Ready);
            }
            Ok(s) => {
                let msg = format!("pip exited with status {}", s.code().unwrap_or(-1));
                send_l(&msg);
                send_s(SetupStatus::Error(msg));
            }
            Err(e) => send_s(SetupStatus::Error(e.to_string())),
        }
    });
}

// ── Process lifecycle ─────────────────────────────────────────────────────────

fn has_gpu() -> bool {
    // Ask the venv's own Python whether torch.cuda is actually available.
    // nvidia-smi being present is not enough — the installed torch may be CPU-only.
    let python = venv_dir().join("bin").join("python");
    Command::new(python)
        .args([
            "-c",
            "import torch, sys; sys.exit(0 if torch.cuda.is_available() else 1)",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn start(id: usize, folder_path: String, port: u16, tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        let send = |s: InstanceStatus| {
            let _ = tx.send(LamaMsg::Status { id, status: s });
        };
        let send_l = |l: String| {
            let _ = tx.send(LamaMsg::LogLine { id, line: l });
        };

        let bin = lama_bin();
        if !bin.exists() {
            send(InstanceStatus::Error(
                "lama-cleaner not installed. Use the Install button in settings.".into(),
            ));
            return;
        }

        let device = if has_gpu() { "cuda" } else { "cpu" };
        let mdir = model_dir();
        let _ = std::fs::create_dir_all(&mdir);
        let mdir_s = mdir.to_string_lossy().into_owned();
        let port_s = port.to_string();

        let args = [
            "--host",
            "127.0.0.1",
            "--port",
            &port_s,
            "--model",
            "lama",
            "--device",
            device,
            "--model-dir",
            &mdir_s,
            "--input",
            &folder_path,
            "--output-dir",
            &folder_path,
            "--quality",
            "95",
        ];

        send_l(format!("CMD: {} {}", bin.display(), args.join(" ")));

        let mut child = match Command::new(&bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                send(InstanceStatus::Error(format!("spawn failed: {e}")));
                return;
            }
        };

        let pid = child.id();
        let _ = tx.send(LamaMsg::Pid { id, pid });

        // Stream stdout live; capture stderr into a shared buffer AND stream it.
        let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        if let Some(r) = child.stdout.take() {
            let tx2 = tx.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(r).lines().map_while(Result::ok) {
                    let _ = tx2.send(LamaMsg::LogLine { id, line });
                }
            });
        }
        if let Some(r) = child.stderr.take() {
            let tx2 = tx.clone();
            let buf2 = std::sync::Arc::clone(&stderr_buf);
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(r).lines().map_while(Result::ok) {
                    let _ = tx2.send(LamaMsg::LogLine {
                        id,
                        line: line.clone(),
                    });
                    if let Ok(mut v) = buf2.lock() {
                        v.push(line);
                    }
                }
            });
        }

        // Poll until the HTTP server accepts connections (model load can take ~30 s).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                send(InstanceStatus::Error(
                    "Server did not respond within 3 minutes.".into(),
                ));
                return;
            }
            // Port bound by someone else → server is up.
            if TcpListener::bind(("127.0.0.1", port)).is_err() {
                send_l(format!("Server ready — http://127.0.0.1:{port}"));
                send(InstanceStatus::Running);
                break;
            }
            if let Ok(Some(code)) = child.try_wait() {
                // Wait briefly so stderr reader thread can flush its lines.
                std::thread::sleep(std::time::Duration::from_millis(300));
                let tail = stderr_buf
                    .lock()
                    .map(|v| {
                        v.iter()
                            .rev()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .unwrap_or_default();
                let detail = if tail.is_empty() {
                    String::new()
                } else {
                    format!(": {tail}")
                };
                send(InstanceStatus::Error(format!(
                    "Process exited early (code {}){detail}",
                    code.code().unwrap_or(-1)
                )));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        // Block here; the thread exits (and sends Stopped) when the process dies.
        let _ = child.wait();
        let _ = tx.send(LamaMsg::Status {
            id,
            status: InstanceStatus::Stopped,
        });
    });
}

pub fn stop(id: usize, pid: u32, folder_path: String, tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        if pid > 0 {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output();
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .output();
        }

        let thumbs = std::path::Path::new(&folder_path).join(".thumbnails");
        if thumbs.is_dir() {
            let _ = std::fs::remove_dir_all(&thumbs);
        }

        let _ = tx.send(LamaMsg::Status {
            id,
            status: InstanceStatus::Stopped,
        });
    });
}

pub fn restart(id: usize, pid: u32, folder_path: String, port: u16, tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        // Kill old process first, then start fresh.
        if pid > 0 {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output();
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .output();
        }
        // Delegate to start — it sends Pid + Running/Error back.
        start(id, folder_path, port, tx);
    });
}

pub fn poll_status(id: usize, pid: Option<u32>, tx: mpsc::Sender<LamaMsg>) {
    std::thread::spawn(move || {
        let alive = pid.map_or(false, |p| {
            std::path::Path::new(&format!("/proc/{p}")).exists()
        });
        let status = if alive {
            InstanceStatus::Running
        } else {
            InstanceStatus::Stopped
        };
        let _ = tx.send(LamaMsg::Status { id, status });
    });
}

// ── Browser launch ────────────────────────────────────────────────────────────

pub fn open_in_browser(url: &str) {
    let _ = Command::new("xdg-open").arg(url).spawn();
}

pub fn open_in_app_window(url: &str) {
    let url = url.to_owned();
    let app_arg = format!("--app={}", url);
    let sizes = ["--window-size=1280,900", "--window-position=100,50"];

    for browser in &[
        "chromium-browser",
        "chromium",
        "google-chrome",
        "google-chrome-stable",
        "brave-browser",
    ] {
        if Command::new(browser)
            .arg(&app_arg)
            .args(sizes)
            .spawn()
            .is_ok()
        {
            return;
        }
    }
    if Command::new("firefox")
        .args(["--new-window", &url])
        .spawn()
        .is_ok()
    {
        return;
    }
    let _ = Command::new("xdg-open").arg(&url).spawn();
}
