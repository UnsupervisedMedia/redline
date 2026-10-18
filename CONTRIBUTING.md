# Contributing

Thanks for helping. Redline is a Rust + egui app; Linux is the primary development platform.

## Build and test
```sh
cargo build
cargo test
cargo fmt --all
cargo clippy --all-targets -- -D clippy::correctness -D clippy::suspicious
cargo deny check          # needs cargo-deny
```
Linux build dependencies: `libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libssl-dev pkg-config`.
Optional features (`native-inpaint`, `native-generative`, `pdf`, `lama-webview`, `local-ai`) have extra requirements; see the README.

## Pull requests
- Branch from `main`, open a PR; `main` requires review by a code owner and a green `ci-gate` check.
- Keep PRs focused. Add or update tests for behavior changes.
- Do not add assets (icons, fonts, images, models) without recording their source and license in `docs/LICENSING.md`. Unknown provenance will be rejected.
- Do not add dependencies with GPL/AGPL licenses; `cargo deny` will fail.
- By contributing you agree your work is licensed under Apache-2.0.
