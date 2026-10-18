#!/bin/bash
# Install RedLine as a desktop application.
#
#   ./install.sh              Fast incremental build from the current working tree
#                             (your everyday dev loop — may be a -dirty build).
#   ./install.sh --tag vX.Y.Z Clean build from a git tag in an isolated worktree,
#                             for cutting a real release. Slower (full from-scratch
#                             compile, no shared target cache) but guaranteed to come
#                             from exactly that committed, tagged state.
set -e

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
SRC_ROOT="$REPO_ROOT"   # where we build from and read assets from
TAG=""
WORKTREE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --tag)   TAG="${2:?--tag requires a tag name}"; shift 2 ;;
        --tag=*) TAG="${1#--tag=}"; shift ;;
        -h|--help)
            echo "Usage: ./install.sh [--tag <tag>]"
            exit 0 ;;
        *) echo "Error: unknown argument '$1' (try --help)"; exit 1 ;;
    esac
done

# A tagged build is done in a throwaway worktree so it can't pick up anything from
# your working tree; remove it on exit regardless of success/failure.
cleanup_worktree() {
    [ -n "$WORKTREE" ] && git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" 2>/dev/null || true
}

if [ -n "$TAG" ]; then
    if ! git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$TAG^{commit}" >/dev/null; then
        echo "Error: tag '$TAG' not found. Known tags:"
        git -C "$REPO_ROOT" tag | tail -n 10
        exit 1
    fi
    WORKTREE="$(mktemp -d "${TMPDIR:-/tmp}/redline-build.XXXXXX")"
    trap cleanup_worktree EXIT
    echo "Checking out tag '$TAG' into isolated worktree: $WORKTREE"
    git -C "$REPO_ROOT" worktree add --detach --force "$WORKTREE" "refs/tags/$TAG" >/dev/null
    SRC_ROOT="$WORKTREE"
fi

echo "Building RedLine (release mode, features=agent-server,local-ai,native-inpaint,native-generative)..."
# Build inside SRC_ROOT so build.rs's `git describe` resolves against the right tree
# (a worktree on the tag stamps a clean vX.Y.Z; the working tree stamps -dirty).
( cd "$SRC_ROOT" && cargo build --release --features "agent-server,local-ai,native-inpaint,native-generative" )

BINARY="$SRC_ROOT/target/release/redline"
if [ ! -f "$BINARY" ]; then
    echo "Error: Build failed, binary not found at $BINARY"
    exit 1
fi
echo "Built $("$BINARY" --version 2>/dev/null || echo "RedLine")"

# Install the real binary into the lib dir; a generated wrapper on PATH (written
# below, after ORT detection) sets the ONNX Runtime env and execs it — so both
# terminal `redline` and the menu launcher get the same GPU/CPU runtime.
INSTALL_DIR="$HOME/.local/bin"
REAL_BIN_DIR="$HOME/.local/lib/redline"
mkdir -p "$INSTALL_DIR" "$REAL_BIN_DIR"
TMP_BINARY="$(mktemp "$REAL_BIN_DIR/redline.tmp.XXXXXX")"
cp "$BINARY" "$TMP_BINARY"
chmod +x "$TMP_BINARY"
mv -f "$TMP_BINARY" "$REAL_BIN_DIR/redline"
echo "Installed binary to $REAL_BIN_DIR/redline"

# Vendor ONNX Runtime 1.22.x for the Native Inpaint engine.
# `ort` loads libonnxruntime.so dynamically and needs EXACTLY 1.22.x (1.14/1.20/1.23+
# are ABI-incompatible). Prefer a GPU-capable build (one whose directory also ships
# libonnxruntime_providers_cuda.so) so CUDA-capable machines run on the GPU; otherwise fall
# back to a CPU build. Vendor the lib(s) into a stable dir and wire the launcher.
ORT_LIB_DIR="$HOME/.local/lib/redline"
ORT_DEST="$ORT_LIB_DIR/libonnxruntime.so"
ORT_DYLIB=""   # value for ORT_DYLIB_PATH in the wrapper (empty = not vendored)
ORT_LD=""      # extra LD_LIBRARY_PATH dirs for the wrapper (GPU only)
ORT_SEARCH_ROOTS=("$HOME/.cache/uv" "$HOME/.local/lib" "$HOME/.venv" "$HOME/.local/share/redline" /usr/local/lib /usr/lib)

# Find a GPU-capable 1.22.x libonnxruntime.so (dir also has the CUDA provider lib).
find_ort_122_gpu() {
    local lib dir
    while IFS= read -r lib; do
        dir="$(dirname "$lib")"
        if ls "$dir"/libonnxruntime_providers_cuda.so* >/dev/null 2>&1; then
            echo "$lib"; return 0
        fi
    done < <(find "${ORT_SEARCH_ROOTS[@]}" -name 'libonnxruntime.so.1.22*' 2>/dev/null)
    return 1
}

# Find any 1.22.x libonnxruntime.so (CPU fallback).
find_ort_122_cpu() {
    find "${ORT_SEARCH_ROOTS[@]}" -name 'libonnxruntime.so.1.22*' 2>/dev/null | head -n1
}

ORT_IS_GPU=0
SRC_ORT=""
NV_DIRS=""   # extra CUDA/cuDNN lib dirs (pip/venv layout) for LD_LIBRARY_PATH

# Prefer a GPU build (re-derived each run so a fresh onnxruntime-gpu venv is picked
# up); then an explicit override; then a previously vendored lib; then CPU.
GPU_LIB="$(find_ort_122_gpu || true)"
if [ -n "$GPU_LIB" ]; then
    SRC_ORT="$GPU_LIB"
    ORT_IS_GPU=1
    # In a pip/venv layout the CUDA/cuDNN libs live at <site-packages>/nvidia/*/lib;
    # libonnxruntime.so.* dlopens libcudnn.so.9 etc. from there at runtime.
    sp="${GPU_LIB%%/onnxruntime/capi/*}"
    [ -d "$sp/nvidia" ] && NV_DIRS="$(find "$sp/nvidia" -maxdepth 2 -name lib -type d 2>/dev/null | paste -sd: -)"
elif [ -n "$ORT_DYLIB_PATH" ] && [ -f "$ORT_DYLIB_PATH" ]; then
    SRC_ORT="$ORT_DYLIB_PATH"
elif [ -f "$ORT_DEST" ]; then
    SRC_ORT="$ORT_DEST"   # previously vendored
    [ -f "$ORT_LIB_DIR/libonnxruntime_providers_cuda.so" ] && ORT_IS_GPU=1
else
    SRC_ORT="$(find_ort_122_cpu || true)"
fi

if [ -n "$SRC_ORT" ]; then
    mkdir -p "$ORT_LIB_DIR"
    [ "$SRC_ORT" != "$ORT_DEST" ] && cp -f "$SRC_ORT" "$ORT_DEST"
    if [ "$ORT_IS_GPU" = "1" ]; then
        # Bring the CUDA/TensorRT provider + shared libs alongside so ORT can load them.
        src_dir="$(dirname "$SRC_ORT")"
        cp -f "$src_dir"/libonnxruntime_providers_*.so* "$ORT_LIB_DIR"/ 2>/dev/null || true
        ORT_LIBPATH="$ORT_LIB_DIR"
        [ -n "$NV_DIRS" ] && ORT_LIBPATH="$ORT_LIB_DIR:$NV_DIRS"
        echo "Vendored GPU ONNX Runtime for native inpaint (CUDA): $SRC_ORT"
        [ -n "$NV_DIRS" ] && echo "  CUDA/cuDNN lib dirs: $NV_DIRS"
        ORT_DYLIB="$ORT_DEST"
        ORT_LD="$ORT_LIBPATH"
    else
        echo "Vendored CPU ONNX Runtime for native inpaint: $SRC_ORT -> $ORT_DEST"
        echo "  (No GPU build found. For CUDA on this box:"
        echo "     python3 -m venv ~/.local/share/redline/ort-gpu"
        echo "     ~/.local/share/redline/ort-gpu/bin/pip install onnxruntime-gpu==1.22.0 'nvidia-cudnn-cu12>=9,<10'"
        echo "   then re-run ./install.sh)"
        ORT_DYLIB="$ORT_DEST"
    fi
else
    echo "WARNING: No ONNX Runtime 1.22.x found — Native Inpaint will fail to load at runtime."
    echo "         CPU:  pip install onnxruntime==1.22.1"
    echo "         GPU:  pip install onnxruntime-gpu==1.22.0 'nvidia-cudnn-cu12>=9,<10'"
fi

# Write the launcher wrapper on PATH. It sets the ONNX Runtime env (so terminal
# and menu launches both find the runtime) and execs the real binary. LD append
# is real here (shell script, unlike the .desktop Exec field).
WRAPPER="$INSTALL_DIR/redline"
{
    echo '#!/bin/sh'
    echo '# Auto-generated by RedLine install.sh — do not edit (re-run ./install.sh).'
    [ -n "$ORT_DYLIB" ] && echo "export ORT_DYLIB_PATH=\"$ORT_DYLIB\""
    [ -n "$ORT_LD" ] && echo "export LD_LIBRARY_PATH=\"$ORT_LD\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}\""
    echo "exec \"$REAL_BIN_DIR/redline\" \"\$@\""
} > "$WRAPPER.tmp.$$"
chmod +x "$WRAPPER.tmp.$$"
mv -f "$WRAPPER.tmp.$$" "$WRAPPER"
echo "Installed launcher wrapper to $WRAPPER"

# Install icons (optional - skip any size whose file is missing)
for size in 48 128 256; do
    src="$SRC_ROOT/assets/redline-${size}.png"
    if [ -f "$src" ]; then
        dir="$HOME/.local/share/icons/hicolor/${size}x${size}/apps"
        mkdir -p "$dir"
        cp "$src" "$dir/redline.png"
        echo "Installed icon $src -> $dir/redline.png"
    fi
done

# Install scalable SVG if present
if [ -f "$SRC_ROOT/assets/redline.svg" ]; then
    SVG_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
    mkdir -p "$SVG_DIR"
    cp "$SRC_ROOT/assets/redline.svg" "$SVG_DIR/redline.svg"
    echo "Installed scalable icon to $SVG_DIR/redline.svg"
fi

# Install .desktop file
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
cp "$SRC_ROOT/assets/redline.desktop" "$DESKTOP_DIR/redline.desktop"
# Exec=redline resolves to the wrapper on PATH, which sets the ORT env — so the
# menu launcher and terminal share one launch path.
echo "Installed desktop entry to $DESKTOP_DIR/redline.desktop"

# Update icon cache
if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f "$HOME/.local/share/icons/hicolor/" 2>/dev/null || true
fi
if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

echo ""
echo "RedLine installed successfully!"
echo "  Binary: $INSTALL_DIR/redline"
echo "  Desktop: $DESKTOP_DIR/redline.desktop"
echo ""
echo "You should now see 'RedLine' in your application menu."
echo "Make sure $INSTALL_DIR is in your PATH."
