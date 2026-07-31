#!/usr/bin/env bash
# run.sh — compile and launch Vimbatim
#
# Usage:
#   ./run.sh          build (debug) and run
#   ./run.sh --release  build with optimisations and run
#   ./run.sh --check    compile-check only, do not run
#   ./run.sh --help

set -e

RELEASE=""
CHECK_ONLY=0

for arg in "$@"; do
    case "$arg" in
        --release) RELEASE="--release" ;;
        --check)   CHECK_ONLY=1 ;;
        --help)
            echo "Usage: $0 [--release] [--check]"
            echo "  --release   build with optimisations"
            echo "  --check     compile-check only, do not launch the app"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg  (use --help)" >&2
            exit 1
            ;;
    esac
done

# ── Wayland / X11 display ─────────────────────────────────────────────────────
# GPUI on Linux needs a display server. Prefer Wayland; fall back to X11.
if [ -z "$WAYLAND_DISPLAY" ] && [ -z "$DISPLAY" ]; then
    echo "WARNING: Neither WAYLAND_DISPLAY nor DISPLAY is set."
    echo "         The app may fail to open a window."
    echo "         On WSL2 make sure WSLg is running or export DISPLAY=:0"
fi

# ── Vulkan driver hint ────────────────────────────────────────────────────────
# GPUI uses Vulkan (via wgpu/blade) on Linux. If no GPU is available, the
# software (llvmpipe) driver is used automatically. Uncomment the line below
# to force software rendering explicitly:
#   export LIBGL_ALWAYS_SOFTWARE=1

# ── Build ─────────────────────────────────────────────────────────────────────
if [ "$CHECK_ONLY" -eq 1 ]; then
    echo "==> cargo check $RELEASE"
    cargo check $RELEASE
    echo "==> Check passed."
    exit 0
fi

echo "==> cargo build $RELEASE"
cargo build $RELEASE

# ── Bundled defaults placement ────────────────────────────────────────────────
# The user's own settings.conf lives in the per-user data directory
# (~/.vimbatim on macOS/Linux, %APPDATA%\vimbatim on Windows) and is seeded on
# first launch — nothing to place for it here.
#
# default_settings.conf is different: it's the read-only seed, shipped with the
# build and looked up next to the executable (src/state.rs's
# bundled_default_settings_path()). A dev build's executable lives in
# target/{debug,release}/, so copy it there to match the packaged layout —
# otherwise a fresh ~/.vimbatim seeds from the repo-root fallback instead,
# which works but doesn't exercise the path testers actually get.
if [ -n "$RELEASE" ]; then TARGET_DIR="target/release"; else TARGET_DIR="target/debug"; fi
cp default_settings.conf "$TARGET_DIR/default_settings.conf"

# ── Cursor size ───────────────────────────────────────────────────────────────
# GPUI's X11/WSLg backend can double-scale cursors on HiDPI. Setting
# XCURSOR_SIZE to 24 (standard for 96 dpi) corrects the oversized cursor.
export XCURSOR_SIZE=24

# ── Run ───────────────────────────────────────────────────────────────────────
echo "==> Launching Vimbatim..."
if [ -n "$RELEASE" ]; then
    exec ./target/release/vimbatim
else
    exec ./target/debug/vimbatim
fi
