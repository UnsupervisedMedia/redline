use std::sync::mpsc;
use std::time::Instant;

// ── Setup status ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SetupStatus {
    Unknown,
    Checking,
    NeedsInstall,
    Installing,
    Ready,
    Error(String),
}

impl SetupStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

// ── Instance status ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum InstanceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Restarting,
    Error(String),
}

impl InstanceStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting…",
            Self::Running => "Running",
            Self::Stopping => "Stopping…",
            Self::Restarting => "Restarting…",
            Self::Error(_) => "Error",
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Starting | Self::Stopping | Self::Restarting)
    }
}

// ── Messages from background threads ─────────────────────────────────────────

pub enum LamaMsg {
    Status { id: usize, status: InstanceStatus },
    LogLine { id: usize, line: String },
    Pid { id: usize, pid: u32 },
    SetupStatus(SetupStatus),
    SetupLog(String),
}

// ── Per-instance data ─────────────────────────────────────────────────────────

pub struct LamaInstance {
    pub id: usize,
    pub label: String,
    pub folder_path: String,
    pub child_pid: Option<u32>,
    pub port: u16,
    pub status: InstanceStatus,
    pub log_lines: Vec<String>,
    // transient UI state
    pub show_logs: bool,
    pub folder_edit: String,
    pub editing_label: bool,
    pub label_edit: String,
    pub show_embedded: bool,
}

impl LamaInstance {
    pub fn new(id: usize, folder_path: &str, port: u16) -> Self {
        let label = format!("Instance {}", id + 1);
        Self {
            id,
            label: label.clone(),
            label_edit: label,
            folder_path: folder_path.to_owned(),
            folder_edit: folder_path.to_owned(),
            child_pid: None,
            port,
            status: InstanceStatus::Stopped,
            log_lines: Vec::new(),
            show_logs: false,
            editing_label: false,
            show_embedded: false,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

// ── Module-level state held in AppState ──────────────────────────────────────

#[cfg(feature = "lama-webview")]
pub struct EmbeddedWebView {
    pub view: wry::WebView,
    #[allow(dead_code)]
    pub instance_id: usize,
    pub last_rect: Option<egui::Rect>,
}

pub struct LamaCleanerState {
    pub instances: Vec<LamaInstance>,
    pub active_tab: usize,
    pub next_id: usize,
    pub last_poll: Instant,
    pub tx: mpsc::Sender<LamaMsg>,
    pub rx: mpsc::Receiver<LamaMsg>,

    pub setup: SetupStatus,
    pub setup_log: Vec<String>,

    #[cfg(feature = "lama-webview")]
    pub webview: Option<EmbeddedWebView>,

    pub webview_target_rect: Option<egui::Rect>,
    pub open_webview_for: Option<usize>,
    pub close_webview: bool,
}

impl Default for LamaCleanerState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            instances: Vec::new(),
            active_tab: 0,
            next_id: 0,
            last_poll: Instant::now(),
            tx,
            rx,
            setup: SetupStatus::Unknown,
            setup_log: Vec::new(),
            #[cfg(feature = "lama-webview")]
            webview: None,
            webview_target_rect: None,
            open_webview_for: None,
            close_webview: false,
        }
    }
}

impl LamaCleanerState {
    pub fn drain_msgs(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                LamaMsg::Status { id, status } => {
                    if let Some(inst) = self.instances.iter_mut().find(|i| i.id == id) {
                        // Auto-show logs when an error occurs so the user sees the output.
                        if matches!(status, InstanceStatus::Error(_)) {
                            inst.show_logs = true;
                        }
                        inst.status = status;
                    }
                }
                LamaMsg::LogLine { id, line } => {
                    if let Some(inst) = self.instances.iter_mut().find(|i| i.id == id) {
                        inst.log_lines.push(line);
                        // Cap log buffer to prevent unbounded growth.
                        if inst.log_lines.len() > 500 {
                            inst.log_lines.drain(0..100);
                        }
                    }
                }
                LamaMsg::Pid { id, pid } => {
                    if let Some(inst) = self.instances.iter_mut().find(|i| i.id == id) {
                        inst.child_pid = Some(pid);
                    }
                }
                LamaMsg::SetupStatus(s) => {
                    self.setup = s;
                }
                LamaMsg::SetupLog(line) => {
                    self.setup_log.push(line);
                    if self.setup_log.len() > 500 {
                        self.setup_log.drain(0..100);
                    }
                }
            }
        }
    }
}
