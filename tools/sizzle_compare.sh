#!/bin/zsh
# sizzle_compare.sh <out_dir> <layout> <t1,t2,...> [layout times]...
#
# Runs one layout in both engines with the default gameplay camera and no
# input, grabs frames at the same in-game times, and writes
# <out_dir>/<layout>/compare.png: one row per time, C++ (rb) left, Rust right.
#
#   C++  : rb's -record pipeline (-fixedfps 60, -recordat = game seconds after
#          the first 3D frame), frames pulled out of the clip with ffmpeg.
#   Rust : RB_CAPTURE (game/src/capture.rs), same time base, exits by itself.
#
# Logs: <out_dir>/<layout>/{cpp,rs}.log.  A Rust panic / stack overflow is
# summarised in <out_dir>/<layout>/rs_status.txt.
#
# Env: SKIP_CPP=1 re-runs only Rust against an existing cpp.mp4,
#      RB_ROOT (default ~/repos/rb), RB_GAME (default target/debug/rb-game),
#      LIMIT wall seconds per engine (default 240).
set -u
cd "${0:A:h}/.."
OUT="${1:A}"; shift
RB_ROOT="${RB_ROOT:-$HOME/repos/rb}"
RB_GAME="${RB_GAME:-target/debug/rb-game}"
LIMIT="${LIMIT:-240}"
FF="${RB_FFMPEG:-$(python3 -c 'import imageio_ffmpeg as i;print(i.get_ffmpeg_exe())')}"

run_limited() {  # run_limited <log> cmd...   (kill after $LIMIT s)
  local log=$1; shift
  "$@" > "$log" 2>&1 &
  local pid=$!
  for ((i = 0; i < LIMIT; i++)); do
    kill -0 $pid 2>/dev/null || { wait $pid; return $?; }
    /bin/sleep 1
  done
  echo "[compare] wall limit $LIMIT s reached" >> "$log"
  kill -INT $pid 2>/dev/null; /bin/sleep 2; kill -9 $pid 2>/dev/null
  return 124
}

while (( $# >= 2 )); do
  LAYOUT=$1; TIMES=$2; shift 2
  D="$OUT/$LAYOUT"; mkdir -p "$D"; rm -f "$D"/*.png(N)
  times=(${(s:,:)TIMES})
  t0=${times[1]}; tN=${times[-1]}
  secs=$(python3 -c "print(round($tN - $t0 + 0.5, 2))")

  # --- C++ (SKIP_CPP=1 reuses an earlier cpp.mp4) -------------------------
  if [[ -n "${SKIP_CPP:-}" && -f "$D/cpp.mp4" ]]; then :; else
  ( run_limited "$D/cpp.log" "$RB_ROOT/run_rb_macos.sh" -layout "$LAYOUT" -noui -nohud \
      -w 1280 -h 720 -fixedfps 60 -recordfps 30 -record "$D/cpp.mp4" \
      -recordat "$t0" -recordsecs "$secs" -recordffmpeg "$FF"
    echo $? > "$D/cpp_rc" )
  fi
  # --- Rust (after C++: running both at once starved Rust to ~2 fps) ---- --------------------------------------------------------------
  ( RB_CAPTURE="$TIMES" RB_CAPTURE_DIR="$D" RUST_BACKTRACE=1 run_limited "$D/rs.log" \
      "$RB_GAME" --path "$RB_ROOT/assets_unpacked" --dat /nonexistent/RB.DAT --layout "$LAYOUT"
    echo $? > "$D/rs_rc" )

  {
    echo "rc=$(cat "$D/rs_rc")"
    grep -a -m3 -E "panicked|overflowed its stack|fatal runtime" "$D/rs.log"
  } > "$D/rs_status.txt"

  # --- frames + sheet ----------------------------------------------------
  rows=()
  for t in $times; do
    ts=$(printf '%06.2f' $t)
    off=$(python3 -c "print(max(0.0, $t - $t0))")
    cpp="$D/cpp_$ts.png"; rs="$D/rs_$ts.png"
    [[ -f "$D/cpp.mp4" ]] && "$FF" -loglevel error -y -ss "$off" -i "$D/cpp.mp4" -frames:v 1 "$cpp"
    for f in "$cpp" "$rs"; do   # missing frame -> black placeholder
      [[ -f "$f" ]] || "$FF" -loglevel error -y -f lavfi -i color=c=black:s=960x540 -frames:v 1 "$f"
    done
    row="$D/row_$ts.png"
    "$FF" -loglevel error -y -i "$cpp" -i "$rs" -filter_complex \
      "[0]scale=960:540[a];[1]scale=960:540[b];[a][b]hstack" "$row"
    rows+=("$row")
  done
  if (( ${#rows} == 1 )); then cp "$rows[1]" "$D/compare.png"
  else
    ins=(); for r in $rows; do ins+=(-i "$r"); done
    "$FF" -loglevel error -y "${ins[@]}" -filter_complex "vstack=inputs=${#rows}" "$D/compare.png"
  fi
  echo "[compare] $LAYOUT cpp_rc=$(cat "$D/cpp_rc") rs_rc=$(cat "$D/rs_rc") -> $D/compare.png"
done
