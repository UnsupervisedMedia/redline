# Security policy

## Reporting a vulnerability
Please use GitHub's **private vulnerability reporting** (Security tab → "Report a vulnerability"). Do not open public issues for security problems.
We aim to acknowledge within 7 days. This is a small project; there is no bounty program.

## Supported versions
Only the latest release receives fixes.

## Trust model and known limits
- **MCP agent server (opt-in, off by default).** Enabled from the AI / Agents menu. It binds to `127.0.0.1` only (default port 9275), requires a per-process random
  bearer token (published in `~/.local/share/redline/mcp.json`, mode 0600) and rejects any request carrying an `Origin` header. Anyone who can read that file can read and
  edit your open canvas (including a PNG snapshot of it). The tool surface is limited to canvas operations; there are no file-system or command-execution tools.
- **Local AI (Ollama).** The Ollama URL is user-configured. If it is not a loopback address, canvas screenshots are sent to that host (plain `http://` is allowed).
  Text drawn on the canvas reaches the model as input, so treat it as untrusted prompt content; the blast radius is bounded to canvas edits.
- **Network access.** Redline has no telemetry or update checks. It contacts `huggingface.co` (model downloads, only when you use Native Inpaint), PyPI (only if you use the
  optional Lama-Cleaner mode, which `pip install`s a pinned version into a private venv), and your configured Ollama endpoint.
- **Model integrity.** LaMa is SHA-256 verified. Stable Diffusion weights fetched through `hf-hub` track the repository's `main` branch and are not yet hash-pinned (safetensors format, no code execution).
- **Native libraries.** `ORT_DYLIB_PATH` makes Redline `dlopen` the ONNX Runtime library you point it at; whoever controls that variable or file runs code as you.
- **Untrusted files.** Images are decoded with the Rust `image` crate without explicit size limits, so a decompression bomb can exhaust memory. Session files are self-authored data in your user directory.
- **Dependency advisories.** `deny.toml` documents the advisories we have accepted (unmaintained transitive crates from egui 0.29; `quick-xml` only reachable via D-Bus introspection / build-time).
