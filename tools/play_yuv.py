"""Tk-based YUV420P player for eyeballing mpeg2_player_v2 output.

Reads YUV420P frames from a directory (one `frame-NNNNN.yuv` file per frame,
as emitted by `mpeg2_player_v2 --dump-dir`) and plays them in a window at a
target FPS.  Keys: SPACE = pause/resume, LEFT / RIGHT = step one frame
(pauses), R = restart from frame 0, ESC = quit.

Requires Pillow + numpy:
    pip install pillow numpy

Usage:
    python tools/play_yuv.py <dir> --size 512x448 [--fps 29.97]
"""
from __future__ import annotations

import argparse
import os
import sys
import tkinter as tk

import numpy as np
from PIL import Image, ImageTk


def yuv420p_to_rgb(yuv: bytes, w: int, h: int) -> np.ndarray:
    """Vectorised BT.601 (full-range) YUV420P → RGB888.

    The decoder emits 8-bit Y/Cb/Cr already; we treat them as full-range for
    the purposes of eyeball inspection.  Chroma is nearest-neighbour upsampled
    2x.  For ±1 LSB pixel matching against ffmpeg, use the harness instead —
    this script is for visual sanity checks only.
    """
    y_size = w * h
    c_size = (w // 2) * (h // 2)
    y = np.frombuffer(yuv[:y_size], dtype=np.uint8).reshape(h, w).astype(np.int32)
    cb = (
        np.frombuffer(yuv[y_size : y_size + c_size], dtype=np.uint8)
        .reshape(h // 2, w // 2)
        .astype(np.int32)
    )
    cr = (
        np.frombuffer(yuv[y_size + c_size :], dtype=np.uint8)
        .reshape(h // 2, w // 2)
        .astype(np.int32)
    )

    cb = cb.repeat(2, axis=0).repeat(2, axis=1)
    cr = cr.repeat(2, axis=0).repeat(2, axis=1)

    u = cb - 128
    v = cr - 128

    r = y + ((91881 * v + 32768) >> 16)
    g = y - ((22554 * u + 46802 * v + 32768) >> 16)
    b = y + ((116130 * u + 32768) >> 16)

    rgb = np.stack(
        [np.clip(r, 0, 255), np.clip(g, 0, 255), np.clip(b, 0, 255)],
        axis=2,
    ).astype(np.uint8)
    return rgb


def parse_size(s: str) -> tuple[int, int]:
    w, h = s.lower().split("x")
    return int(w), int(h)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("dir", help="dump directory with frame-*.yuv files")
    p.add_argument("--size", required=True, help="e.g. 512x448")
    p.add_argument("--fps", type=float, default=29.97)
    args = p.parse_args()
    w, h = parse_size(args.size)
    frame_size = w * h * 3 // 2

    files = sorted(f for f in os.listdir(args.dir) if f.endswith(".yuv"))
    if not files:
        print(f"no .yuv files in {args.dir}", file=sys.stderr)
        sys.exit(1)
    print(f"playing {len(files)} frames at {args.fps} fps ({w}x{h})")

    root = tk.Tk()
    root.title(f"mpeg2_player_v2 viewer — {os.path.basename(args.dir.rstrip('/\\'))}")
    canvas = tk.Canvas(root, width=w, height=h, bg="black", highlightthickness=0)
    canvas.pack()
    status = tk.Label(root, text="", anchor="w")
    status.pack(fill="x")

    state = {"idx": 0, "playing": True, "photo": None}
    delay_ms = max(1, int(round(1000.0 / args.fps)))

    def show(i: int) -> None:
        i = max(0, min(len(files) - 1, i))
        state["idx"] = i
        path = os.path.join(args.dir, files[i])
        with open(path, "rb") as f:
            data = f.read()
        if len(data) != frame_size:
            status.config(text=f"frame {i}: size mismatch {len(data)} != {frame_size}")
            return
        rgb = yuv420p_to_rgb(data, w, h)
        img = Image.fromarray(rgb)
        # Keep a reference so Tk doesn't garbage-collect it.
        state["photo"] = ImageTk.PhotoImage(img)
        canvas.create_image(0, 0, anchor="nw", image=state["photo"])
        flag = "[playing]" if state["playing"] else "[paused]"
        status.config(text=f"frame {i}/{len(files) - 1}  {flag}  (SPACE pause, ←/→ step, R restart, ESC quit)")

    def tick() -> None:
        if state["playing"]:
            nxt = state["idx"] + 1
            if nxt >= len(files):
                state["playing"] = False
                show(state["idx"])
            else:
                show(nxt)
        root.after(delay_ms, tick)

    def on_key(e: tk.Event) -> None:
        sym = e.keysym
        if sym == "space":
            state["playing"] = not state["playing"]
            show(state["idx"])
        elif sym == "Left":
            state["playing"] = False
            show(state["idx"] - 1)
        elif sym == "Right":
            state["playing"] = False
            show(state["idx"] + 1)
        elif sym in ("r", "R"):
            state["playing"] = True
            show(0)
        elif sym == "Escape":
            root.destroy()

    root.bind("<Key>", on_key)
    show(0)
    root.after(delay_ms, tick)
    root.mainloop()


if __name__ == "__main__":
    main()
