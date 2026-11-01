#!/bin/sh
# Locomotion harness: run rb (C++) and rb-game (Rust) on the same pad script,
# then compare their per-frame gait traces.
#   tools/loco_harness/run.sh [moves.pad] [secs]
# Needs rb.exe built with the -padscript/-gaitlog hooks (repos/rb) and a
# release rb-game.exe.  Logs land in tools/loco_harness/out/.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
PAD=${1:-$HERE/moves.pad}
SECS=${2:-26}
ASSETS=/c/Users/timla/repos/rustexperiment/oni2/zips/assets
RB=/c/Users/timla/repos/rb/age/tools/build/bin/rb.exe
GAME=$HERE/../../target/release/rb-game.exe
OUT=$HERE/out
mkdir -p "$OUT"
# Harness layout: a flat plane + Konoko, nothing else.
cp -r "$HERE/layout/zz_loco_harness" "$ASSETS/layout/"
PADW=$(cygpath -w "$PAD")
ASSETSW=$(cygpath -w "$ASSETS")
echo "C++ ..."
(cd /c/Users/timla/repos/rb/age && timeout $((SECS + 60)) "$RB" -path "$ASSETSW" -noaudio \
    -layout zz_loco_harness -quitafter "$SECS" -padscript "$PADW" -gaitlog > "$OUT/cpp.log" 2>&1) || true
echo "Rust ..."
timeout $((SECS + 15)) "$GAME" --path "$ASSETSW" --layout zz_loco_harness \
    --padscript "$PADW" --gaitlog 2>&1 | sed -u 's/\x1b\[[0-9;]*m//g' > "$OUT/rust.log" || true
rm -rf "$ASSETS/layout/zz_loco_harness"
python "$HERE/compare.py" "$OUT/cpp.log" "$OUT/rust.log" "$PAD"
