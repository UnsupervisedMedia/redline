# Releasing

Versions follow pre-1.0 SemVer: bump the minor for a feature line, the patch for fixes. The source of truth is `version` in `Cargo.toml`.

`build.rs` stamps `git describe --tags --always --dirty` into the binary (`redline --version`, window title). A clean tagged build reports `v0.2.0`; a `-dirty` suffix means a build from an uncommitted tree.

## Cut a release
1. Open a PR that bumps `version` in `Cargo.toml` (and refreshes `Cargo.lock`); merge it once `ci-gate` is green.
2. Tag the merge commit with an annotated tag and push it:
   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```
3. The `Release` workflow builds Linux (x86_64), macOS (arm64) and Windows (x86_64) binaries, writes `SHA256SUMS`, and creates the GitHub release.
   Use "Run workflow" (`workflow_dispatch`) to dry-run the builds without publishing.

## Local install (Linux)
`./install.sh` builds and installs from the working tree; `./install.sh --tag vX.Y.Z` builds a clean copy of a tag in a throwaway worktree.
