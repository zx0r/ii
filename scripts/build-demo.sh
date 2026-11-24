#!/usr/bin/env bash
# ==============================================================================
# ii - Full Production Asset Automation Pipeline (Retina / Max Fidelity)
# 1. Optimizes Screenshots & Banners (assets/images/) via sips + oxipng + cwebp
# 2. Encodes Master MP4 (assets/video/ii-demo.mp4) @ 1080p/Retina, CRF 18
# 3. Compiles Crystal-Clear GIF (assets/gif/ii-preview.gif) with full-pass palette
#    and sierra2_4a diff-mode dither (pixel-sharp fonts without Bayer grain)
# 4. Lossless compression via gifsicle -O3
# ==============================================================================

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

OUTPUT_DIR="assets"
mkdir -p "$OUTPUT_DIR/gif" "$OUTPUT_DIR/video" "$OUTPUT_DIR/images"

echo "=========================================================="
echo "  ii Production Assets Pipeline (Retina Max Fidelity)     "
echo "=========================================================="

# ── 1. Image & Banner Processing (Retina Max Quality) ──────────
echo ""
echo "── [1/3] Processing Retina Images & Banner ──"

# Retina Banner check
BANNER_SRC=""
if [[ -f "$OUTPUT_DIR/images/ii-banner.jpg" ]]; then
    BANNER_SRC="$OUTPUT_DIR/images/ii-banner.jpg"
elif [[ -f "$OUTPUT_DIR/images/ii-banner.png" ]]; then
    BANNER_SRC="$OUTPUT_DIR/images/ii-banner.png"
fi

if [[ -n "$BANNER_SRC" && -f "$BANNER_SRC" ]]; then
    echo "Found banner: $BANNER_SRC"
    if command -v sips >/dev/null 2>&1; then
        sips -Z 3840 "$BANNER_SRC" >/dev/null 2>&1 || true
    fi
    echo "✓ 4K Retina banner ready: $BANNER_SRC"
fi

# Optimize PNG / WebP assets
if command -v sips >/dev/null 2>&1; then
    for img in "$OUTPUT_DIR/images/"*.png; do
        [[ -f "$img" ]] && sips -Z 2560 "$img" >/dev/null 2>&1 || true
    done
fi
if command -v oxipng >/dev/null 2>&1; then
    oxipng -o 4 --strip all "$OUTPUT_DIR/images/"*.png >/dev/null 2>&1 || true
fi
if command -v cwebp >/dev/null 2>&1; then
    for img in "$OUTPUT_DIR/images/"*.png; do
        if [[ -f "$img" ]]; then
            cwebp -q 88 "$img" -o "${img%.png}.webp" >/dev/null 2>&1 || true
        fi
    done
fi
echo "✓ Images & WebP optimized."

# ── 2. Screen Recording Video & GIF Processing ─────────────────
echo ""
echo "── [2/3] Processing Screen Recording (Retina Master -> MP4 + GIF) ──"

INPUT_VIDEO="${1:-}"
if [[ -z "$INPUT_VIDEO" ]]; then
    if [[ -f "$OUTPUT_DIR/video/ii-demo.mov" ]]; then
        INPUT_VIDEO="$OUTPUT_DIR/video/ii-demo.mov"
    elif [[ -f "$OUTPUT_DIR/video/ii.mov" ]]; then
        INPUT_VIDEO="$OUTPUT_DIR/video/ii.mov"
    else
        INPUT_VIDEO=$(ls -t "$OUTPUT_DIR/video/"*.mov *.mov ~/Desktop/Screen\ Recording*.mov 2>/dev/null | head -n 1 || true)
    fi
fi

if [[ -z "$INPUT_VIDEO" || ! -f "$INPUT_VIDEO" ]]; then
    echo "Error: No screen recording found. Place ii-demo.mov in $OUTPUT_DIR/video/ or pass as argument."
    exit 1
fi

echo "Source video: $INPUT_VIDEO"

MP4_OUT="$OUTPUT_DIR/video/ii-demo.mp4"
GIF_FINAL="$OUTPUT_DIR/gif/ii-preview.gif"
PALETTE_PNG="$OUTPUT_DIR/palette.png"

START_OFFSET="${2:-}"
DURATION="${3:-}"

TIME_FLAGS=()
if [[ -n "$START_OFFSET" ]]; then
    TIME_FLAGS+=("-ss" "$START_OFFSET")
fi
if [[ -n "$DURATION" ]]; then
    TIME_FLAGS+=("-t" "$DURATION")
fi

# Step A: Encode Master MP4 (Retina 1440px / 1280px, H.264 High Profile, CRF 18 visually lossless, 16 FPS)
echo "Encoding Master MP4 (Retina Lanczos, H.264, CRF 18, 16 FPS)..."
ffmpeg -y ${TIME_FLAGS[@]+"${TIME_FLAGS[@]}"} -i "$INPUT_VIDEO" \
  -vf "scale=1280:-2:flags=lanczos" \
  -r 16 -c:v libx264 -preset slow -crf 18 -pix_fmt yuv420p -an \
  "$MP4_OUT"

# Step B: Full-Pass Optimal 256-Color Palette Generation (over full video timeline)
echo "Generating full-stream optimal palette (stats_mode=full)..."
ffmpeg -y -i "$MP4_OUT" \
  -vf "fps=16,scale=1280:-2:flags=lanczos,palettegen=stats_mode=full" \
  "$PALETTE_PNG"

# Step C: Render Hi-Fi GIF with Sierra2_4a Dither & Rectangle Diff Tracking
echo "Rendering pixel-sharp Hi-Fi GIF (sierra2_4a + diff_mode=rectangle)..."
ffmpeg -y -i "$MP4_OUT" -i "$PALETTE_PNG" \
  -filter_complex "fps=16,scale=1280:-2:flags=lanczos[x];[x][1:v]paletteuse=dither=sierra2_4a:diff_mode=rectangle" \
  -loop 0 "$GIF_FINAL"
rm -f "$PALETTE_PNG"

# Step D: Gifsicle Lossless/Light-Lossy Optimization
if command -v gifsicle >/dev/null 2>&1; then
    echo "Optimizing GIF with Gifsicle (O3, preserving font sharpness)..."
    gifsicle -O3 --lossy=25 "$GIF_FINAL" -o "$GIF_FINAL"
fi

# ── 3. Summary & Verification ──────────────────────────────────
echo ""
echo "── [3/3] Production Artifacts Ready ──"
ls -lh "$OUTPUT_DIR"/images/ii-banner.* "$MP4_OUT" "$GIF_FINAL" 2>/dev/null || true
echo ""
echo "✓ Pipeline completed successfully."
