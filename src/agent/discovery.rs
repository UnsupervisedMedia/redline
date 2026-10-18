//! Discovery file at ~/.local/share/redline/mcp.json. Agents look here to find
//! the running server's URL and port.

use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct DiscoveryInfo {
    pub url: String,
    pub port: u16,
    pub pid: u32,
    pub token: Option<String>,
}

/// Write the discovery file with mode 0600 on Unix. Returns the path written.
/// Errors are non-fatal (None is returned).
pub fn write(port: u16, token: Option<String>) -> Option<PathBuf> {
    let path = file_path()?;
    let info = DiscoveryInfo {
        url: format!("http://127.0.0.1:{port}"),
        port,
        pid: std::process::id(),
        token,
    };
    let json = serde_json::to_string_pretty(&info).ok()?;
    let parent = path.parent()?;
    std::fs::create_dir_all(parent).ok()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        f.write_all(json.as_bytes()).ok()?;
        f.flush().ok()?;
        f.sync_all().ok()?;
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::File::create(&path).ok()?;
        f.write_all(json.as_bytes()).ok()?;
        f.flush().ok()?;
    }
    Some(path)
}

/// Remove the discovery file. Errors silently — best-effort cleanup.
pub fn remove() {
    if let Some(p) = file_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn file_path() -> Option<PathBuf> {
    // Test-friendly override; if set, write the file at that exact path.
    if let Ok(p) = std::env::var("REDLINE_DISCOVERY_PATH") {
        return Some(PathBuf::from(p));
    }
    let mut p = dirs_next::data_dir()?;
    p.push("redline");
    p.push("mcp.json");
    Some(p)
}
