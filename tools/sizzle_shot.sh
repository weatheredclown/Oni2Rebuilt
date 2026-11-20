#!/bin/zsh
# sizzle_shot.sh <out_dir> <shot> [<shot>...]
#
# Replays rb sizzle-reel keeper shots (~/repos/rb/tools/sizzle) in both
# engines and writes <out_dir>/<shot>/compare.png: one row per time, C++ (rb)
# left, Rust right.
#
#   shots/<shot>.cmd    the shoot.sh line: layout, record_at, secs, extra rb args
#   shots/<shot>.cine   camera path   -> rb -cinecam   / Rust --cinecam
#   shots/<shot>.pad    pad input     -> rb -padscript / Rust --padscript
#   shots/<shot>.aim    stick aiming  -> rb -padaim    / Rust --padaim
#
# Frames are taken at NFRAMES evenly spread times inside the shot window
# [record_at, record_at + secs] (in-game seconds after the first 3D frame).
#
#   C++  : the existing reference clip tools/sizzle/clips/<shot>.mp4 (its t=0 is
#          record_at) when present, else a fresh rb -record of the window
#          (RECORD_CPP=1 forces that).
#   Rust : --fixedfps $RS_FPS --nohud + RB_CAPTURE (game/src/capture.rs, same
#          time base), exits by itself after the last frame.
#
# Logs: <out_dir>/<shot>/{cpp,rs}.log; Rust panics in rs_status.txt.
#
# Env: SIZZLE (default ~/repos/rb/tools/sizzle), RB_ROOT (~/repos/rb),
#      RB_GAME (target/release/rb-game), LIMIT wall seconds per engine (900),
#      NFRAMES (4), RS_FPS (60, rb's shoot.sh -fixedfps), RECORD_CPP, SKIP_RS=1
#      (re-stitch from existing rs_*.png).
set -u
cd "${0:A:h}/.."
OUT="${1:A}"; shift
RB_ROOT="${RB_ROOT:-$HOME/repos/rb}"
SIZZLE="${SIZZLE:-$RB_ROOT/tools/sizzle}"
RB_GAME="${RB_GAME:-target/release/rb-game}"
LIMIT="${LIMIT:-900}"
NFRAMES="${NFRAMES:-4}"
RS_FPS="${RS_FPS:-60}"
FF="${RB_FFMPEG:-$(python3 -c 'import imageio_ffmpeg as i;print(i.get_ffmpeg_exe())')}"

run_limited() {  # run_limited <log> cmd...   (kill after $LIMIT s)
  local log=$1; shift
  "$@" > "$log" 2>&1 &
  local pid=$!
  for ((i = 0; i < LIMIT; i++)); do
    kill -0 $pid 2>/dev/null || { wait $pid; return $?; }
    /bin/sleep 1
  done
  echo "[shot] wall limit $LIMIT s reached" >> "$log"
  kill -INT $pid 2>/dev/null; /bin/sleep 2; kill -9 $pid 2>/dev/null
  return 124
}

for SHOT in "$@"; do
  CMD="$SIZZLE/shots/$SHOT.cmd"
  [[ -f "$CMD" ]] || { echo "[shot] no $CMD"; continue; }
  # "<path>/shoot.sh <shot> <layout> <record_at> <secs> [extra rb args...]"
  words=(${=$(head -1 "$CMD")})
  LAYOUT=${words[3]}; AT=${words[4]}; SECS=${words[5]}; EXTRA=(${words[6,-1]})
  D="$OUT/$SHOT"; mkdir -p "$D"
  [[ -n "${SKIP_RS:-}" ]] || rm -f "$D"/rs_*.png(N)
  rm -f "$D"/cpp_*.png(N) "$D"/row_*.png(N)
  # NFRAMES times spread over the window, clear of its very ends
  times=(${=$(python3 -c "
n=$NFRAMES; a=$AT; s=$SECS
print(' '.join('%.2f' % (a + s * (i + 0.5) / n) for i in range(n)))")})
  TIMES=${(j:,:)times}

  rbargs=(); rsargs=()
  for ext flag in cine cinecam pad padscript aim padaim; do
    f="$SIZZLE/shots/$SHOT.$ext"
    [[ -f "$f" ]] && { rbargs+=(-$flag "$f"); rsargs+=(--$flag "$f"); }
  done
  echo "[shot] $SHOT: $LAYOUT window $AT+$SECS s, frames at $TIMES; ${rsargs[*]:-no scripts}"

  # --- C++ ---------------------------------------------------------------
  CPP="$SIZZLE/clips/$SHOT.mp4"; CPP_AT=$AT
  if [[ -n "${RECORD_CPP:-}" || ! -f "$CPP" ]]; then
    CPP="$D/cpp.mp4"
    run_limited "$D/cpp.log" "$RB_ROOT/run_rb_macos.sh" -layout "$LAYOUT" -noui -nohud \
      -w 1280 -h 720 -fixedfps 60 -recordfps 30 -record "$CPP" \
      -recordat "$AT" -recordsecs "$SECS" -recordffmpeg "$FF" "${rbargs[@]}" "${EXTRA[@]}"
    echo "rc=$?" > "$D/cpp_status.txt"
  else
    echo "reference clip $CPP" > "$D/cpp_status.txt"
  fi

  # --- Rust --------------------------------------------------------------
  if [[ -z "${SKIP_RS:-}" ]]; then
    RB_CAPTURE="$TIMES" RB_CAPTURE_DIR="$D" RUST_BACKTRACE=1 run_limited "$D/rs.log" \
      "$RB_GAME" --path "$RB_ROOT/assets_unpacked" --dat /nonexistent/RB.DAT --layout "$LAYOUT" \
      --fixedfps "$RS_FPS" --nohud "${rsargs[@]}"
    rc=$?
    { echo "rc=$rc"; grep -a -m3 -E "panicked|overflowed its stack|fatal runtime" "$D/rs.log" } \
      > "$D/rs_status.txt"
  fi

  # --- frames + sheet ----------------------------------------------------
  rows=()
  for t in $times; do
    ts=$(printf '%06.2f' $t)
    off=$(python3 -c "print(max(0.0, $t - $CPP_AT))")
    cpp="$D/cpp_$ts.png"; rs="$D/rs_$ts.png"
    [[ -f "$CPP" ]] && "$FF" -loglevel error -y -ss "$off" -i "$CPP" -frames:v 1 "$cpp"
    for f in "$cpp" "$rs"; do   # missing frame -> black placeholder
      [[ -f "$f" ]] || "$FF" -loglevel error -y -f lavfi -i color=c=black:s=960x540 -frames:v 1 "$f"
    done
    row="$D/row_$ts.png"
    "$FF" -loglevel error -y -i "$cpp" -i "$rs" -filter_complex \
      "[0]scale=960:540,setsar=1[a];[1]scale=960:540,setsar=1[b];[a][b]hstack" "$row"
    rows+=("$row")
  done
  if (( ${#rows} == 1 )); then cp "$rows[1]" "$D/compare.png"
  else
    ins=(); for r in $rows; do ins+=(-i "$r"); done
    "$FF" -loglevel error -y "${ins[@]}" -filter_complex "vstack=inputs=${#rows}" "$D/compare.png"
  fi
  echo "[shot] $SHOT $(cat "$D/cpp_status.txt") / rust $(head -1 "$D/rs_status.txt" 2>/dev/null) -> $D/compare.png"
done
