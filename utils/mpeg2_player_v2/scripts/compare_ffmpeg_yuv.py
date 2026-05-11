#!/usr/bin/env python3
"""Compare mpeg2_player_v2 YUV dumps against ffmpeg golden output.

This is the golden-frame harness for decoder milestones 4+. It generates (or
accepts) MPEG-2 fixtures, runs ffmpeg to create temporary YUV420P references,
runs the crate CLI to produce `frame-*.yuv` files, and compares every byte with
±1 LSB tolerance.

Use `--check-fixtures-only` in environments where the decoder is not expected
to be complete yet; that mode verifies ffmpeg can decode the generated/provided
fixtures and that the reference frame count is non-zero without invoking the
player.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TOLERANCE = 1


def repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def run(command: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise SystemExit(f"required tool '{name}' was not found on PATH")


def generate_progressive_simple(out: Path) -> Path:
    fixture = out / "progressive_simple.m2v"
    proc = run([
        "ffmpeg",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=16x16:rate=1:duration=1",
        "-frames:v",
        "1",
        "-pix_fmt",
        "yuv420p",
        "-c:v",
        "mpeg2video",
        "-g",
        "1",
        "-bf",
        "0",
        "-q:v",
        "2",
        "-f",
        "mpeg2video",
        "-y",
        str(fixture),
    ])
    if proc.returncode != 0:
        raise SystemExit(f"failed to generate {fixture}:\n{proc.stderr}")
    return fixture


def probe_dimensions(fixture: Path) -> tuple[int, int]:
    proc = run([
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height",
        "-of",
        "default=nokey=1:noprint_wrappers=1",
        str(fixture),
    ])
    if proc.returncode != 0:
        raise SystemExit(f"ffprobe failed for {fixture}:\n{proc.stderr}")
    values = [line.strip() for line in proc.stdout.splitlines() if line.strip()]
    if len(values) < 2:
        raise SystemExit(f"ffprobe did not report width and height for {fixture}: {proc.stdout!r}")
    return int(values[0]), int(values[1])


def ffmpeg_reference(fixture: Path, out: Path) -> tuple[int, int, int]:
    width, height = probe_dimensions(fixture)
    proc = run([
        "ffmpeg",
        "-v",
        "error",
        "-i",
        str(fixture),
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "-y",
        str(out),
    ])
    if proc.returncode != 0:
        raise SystemExit(f"ffmpeg failed for {fixture}:\n{proc.stderr}")
    frame_size = width * height * 3 // 2
    size = out.stat().st_size
    if frame_size == 0 or size == 0 or size % frame_size != 0:
        raise SystemExit(
            f"ffmpeg output for {fixture} has invalid size {size}; frame size is {frame_size}"
        )
    return width, height, size // frame_size


def run_player(fixture: Path, dump_dir: Path) -> None:
    proc = run(
        ["cargo", "run", "-q", "-p", "mpeg2_player_v2", "--", str(fixture), "--dump-dir", str(dump_dir)],
        cwd=repo_root(),
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"mpeg2_player_v2 failed for {fixture}:\nSTDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
        )


def compare_file(reference: bytes, actual: bytes, label: str) -> int:
    mismatches = 0
    first = None
    for idx, (expected, got) in enumerate(zip(reference, actual)):
        if abs(expected - got) > TOLERANCE:
            mismatches += 1
            if first is None:
                first = (idx, expected, got)
    if mismatches and first is not None:
        idx, expected, got = first
        print(f"{label}: {mismatches} mismatches; first byte {idx}: expected {expected}, got {got}")
    return mismatches


def compare_fixture(fixture: Path, check_fixtures_only: bool) -> None:
    with tempfile.TemporaryDirectory(prefix="mpeg2_player_v2_") as temp_name:
        temp = Path(temp_name)
        reference_path = temp / "ffmpeg.yuv"
        width, height, frame_count = ffmpeg_reference(fixture, reference_path)
        frame_size = width * height * 3 // 2
        print(f"{fixture}: ffmpeg decoded {frame_count} frame(s) at {width}x{height}")
        if check_fixtures_only:
            return

        decoded_dir = temp / "player"
        decoded_dir.mkdir()
        run_player(fixture, decoded_dir)
        decoded_frames = sorted(decoded_dir.glob("frame-*.yuv"))
        if len(decoded_frames) != frame_count:
            raise SystemExit(
                f"{fixture}: expected {frame_count} decoded player frame(s), found {len(decoded_frames)} in {decoded_dir}"
            )

        reference = reference_path.read_bytes()
        total_mismatches = 0
        for index, decoded in enumerate(decoded_frames):
            actual = decoded.read_bytes()
            expected = reference[index * frame_size : (index + 1) * frame_size]
            if len(actual) != frame_size:
                raise SystemExit(f"{decoded}: expected {frame_size} bytes, found {len(actual)}")
            total_mismatches += compare_file(expected, actual, f"{fixture.name} frame {index}")
        if total_mismatches:
            raise SystemExit(f"{fixture}: {total_mismatches} byte(s) exceeded ±{TOLERANCE} LSB tolerance")
        print(f"{fixture}: player output matches ffmpeg within ±{TOLERANCE} LSB")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "fixtures",
        nargs="*",
        type=Path,
        help="MPEG-2 fixtures to compare; defaults to a generated progressive_simple.m2v",
    )
    parser.add_argument(
        "--check-fixtures-only",
        action="store_true",
        help="only verify ffmpeg can decode fixtures; do not run the unfinished player",
    )
    parser.add_argument(
        "--generate-progressive-simple",
        action="store_true",
        help="also generate the tiny progressive_simple.m2v fixture in a temporary directory",
    )
    args = parser.parse_args()

    require_tool("ffmpeg")
    require_tool("ffprobe")
    if not args.check_fixtures_only:
        require_tool("cargo")

    with tempfile.TemporaryDirectory(prefix="mpeg2_player_v2_fixtures_") as generated_name:
        generated = Path(generated_name)
        fixtures = [fixture.resolve() for fixture in args.fixtures]
        if args.generate_progressive_simple or not fixtures:
            fixtures.append(generate_progressive_simple(generated))
        for fixture in fixtures:
            compare_fixture(fixture, args.check_fixtures_only)
    return 0


if __name__ == "__main__":
    sys.exit(main())
