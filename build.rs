use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    emit_version();

    let stamp_dir = Path::new("icons/stamps");
    println!("cargo:rerun-if-changed={}", stamp_dir.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let out_file = out_dir.join("stamp_defs.rs");

    let mut entries: Vec<_> = fs::read_dir(stamp_dir)
        .expect("read New Shapes dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("svg"))
        .collect();
    entries.sort();

    let mut generated = String::from("&[\n");
    for path in entries {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("stamp file name");
        let stem = path
            .file_stem()
            .and_then(|n| n.to_str())
            .expect("stamp file stem");
        let id = sanitize_id(stem);
        let label = display_label(stem);
        let category = infer_category(&id);
        generated.push_str(&format!(
            "    StampDef {{ id: {:?}, label: {:?}, category: {:?}, uri: {:?}, svg_bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/icons/stamps/{}\")) }},\n",
            id,
            label,
            category,
            format!("bytes://stamp/{id}.svg"),
            escape_for_rust_string(file_name),
        ));
    }
    generated.push_str("]\n");

    fs::write(out_file, generated).expect("write stamp defs");

    if std::env::var("CARGO_FEATURE_NATIVE_INPAINT").is_ok() {
        println!("cargo:warning=native-inpaint: ensure ONNX Runtime 1.22.x is installed (set ORT_DYLIB_PATH if libonnxruntime.so is not on the default loader path)");
    }
}

/// Stamp a version string into the binary as the `REDLINE_VERSION` env var
/// (read at compile time via `env!`). Prefers `git describe` so the binary
/// reports its exact source state — e.g. `v0.2.0` from a tagged release, or
/// `v0.2.0-3-g50c9393-dirty` from a working-tree build with uncommitted
/// changes. Falls back to the Cargo package version when git is unavailable
/// (e.g. building from a source tarball with no `.git`).
fn emit_version() {
    // Re-stamp when HEAD moves (new commit / checkout / tag). We can't detect a
    // freshly-dirtied tree this way, but the common case — building right after a
    // commit or tag — stays accurate, and a clean release build is always correct.
    for p in [".git/HEAD", ".git/index"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }

    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let version = match describe {
        // `git describe` already prefixes tags with `v`; an untagged repo returns
        // a bare short hash, so label it with the Cargo version for context.
        Some(d) if d.starts_with('v') => d,
        Some(d) => format!("v{pkg}-g{d}"),
        None => format!("v{pkg}"),
    };
    println!("cargo:rustc-env=REDLINE_VERSION={version}");
}

fn sanitize_id(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    let mut last_dash = false;
    for ch in stem.chars().flat_map(char::to_lowercase) {
        let keep = ch.is_ascii_alphanumeric();
        if keep {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn display_label(stem: &str) -> String {
    stem.split(|c: char| c == '-' || c == '_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_uppercase().collect::<String>();
                    word.push_str(chars.as_str());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn infer_category(id: &str) -> &'static str {
    let has = |needle: &str| id.contains(needle);
    if [
        "arrow",
        "caret",
        "diagonal",
        "downleft",
        "downright",
        "slim-",
        "go-",
        "jump",
        "circle-up",
    ]
    .iter()
    .any(|needle| has(needle))
    {
        "Arrows & Direction"
    } else if [
        "box",
        "square",
        "circle",
        "octagon",
        "triangle",
        "polygon",
        "parallelogram",
        "star",
    ]
    .iter()
    .any(|needle| has(needle))
    {
        "Geometry & Frames"
    } else if ["anchor", "attachment", "link", "chain"]
        .iter()
        .any(|needle| has(needle))
    {
        "Links & Anchors"
    } else if [
        "bomb", "bug", "fire", "fix", "check", "deny", "times", "question", "help", "knife", "cut",
        "scissors", "spanner", "tool", "key", "puzzle", "plus", "minus", "equals", "divide",
    ]
    .iter()
    .any(|needle| has(needle))
    {
        "Warnings & Symbols"
    } else if [
        "zoom",
        "brightness",
        "contrast",
        "format-",
        "open-menu",
        "selection",
        "preferences",
        "chart",
        "compress",
        "th-",
        "network",
        "find-location",
        "hourglass",
        "playlist",
    ]
    .iter()
    .any(|needle| has(needle))
    {
        "UI & Status"
    } else if [
        "speaker",
        "document",
        "folder",
        "image",
        "envelope",
        "paper-plane",
        "emblem",
        "game",
        "input-",
        "eye",
        "ear",
        "heart",
        "bulb",
        "bright",
        "sun",
        "water",
    ]
    .iter()
    .any(|needle| has(needle))
    {
        "Objects & Icons"
    } else if ["face-", "rage-"].iter().any(|needle| has(needle)) {
        "Faces & Reactions"
    } else {
        "Misc"
    }
}

fn escape_for_rust_string(input: &str) -> String {
    input.replace('\\', "\\\\").replace('\"', "\\\"")
}
