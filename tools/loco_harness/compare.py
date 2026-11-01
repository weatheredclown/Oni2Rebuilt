#!/usr/bin/env python3
"""Compare rb (C++) and rb-game (Rust) locomotion gait traces.

Both builds print one `[gait]` line per frame when run with the harness
flags (see README.md).  This script lines the two up on the first frame the
stick leaves neutral, then for every pad-script step reports, per build:

  * family  - what the legs are doing once settled: STAND, FWD, BACK,
              STRAFE_L, STRAFE_R (the highest-weighted family in the blend)
  * settle  - seconds from the step's start until that family is reached
  * value / sub - the main gait value and the strafe sub-blender value
  * flips   - how many times the family changed inside the step (a kink
              metric: legs switching between unrelated cycles)

Usage: compare.py <cpp.log> <rust.log> <moves.pad>
"""
import re
import sys

LINE = re.compile(r"\[gait\] t=([\d.]+) stick=\(([-\d.]+),([-\d.]+)\) yaw=([-\d.]+) pos=\(([-\d.]+),([-\d.]+)\) fs=(\d) (.*)")
REC = re.compile(r"\[(\d+):([^\s\]]+) s=([\d.]+)")
SUB = re.compile(r"\[\d+:sub v=([-\d.]+)")
VAL = re.compile(r"^v=([-\d.]+)")


def family_of(name: str) -> str:
    n = name.lower()
    if "straf" in n or "strafe" in n:
        if "lft" in n or "left" in n:
            return "STRAFE_L"
        if "rht" in n or "right" in n:
            return "STRAFE_R"
    if "bak" in n or "back" in n:
        return "BACK"
    if "stn" in n or "stand" in n or "fight" in n:
        return "STAND"
    return "FWD"


def parse(path):
    rows = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = LINE.search(line)
            if not m:
                continue
            t, lx, ly, yaw, px, pz, fs, body = m.groups()
            v = VAL.search(body)
            s = SUB.search(body)
            # The pose is built from the highest fully-weighted gait upward,
            # and the strafe sub-blender draws last, so: strafe wins once its
            # top record is weighted >= 0.5; else the highest main gait.
            main = body.split("[6:sub")[0] if "[6:sub" in body else body
            sub_part = body[len(main):]
            fam = "STAND"
            best = -1
            for idx, name, sc in REC.findall(main):
                if float(sc) > 0.0 and int(idx) > best:
                    best, fam = int(idx), family_of(name)
            sub_recs = [(n, float(sc)) for _, n, sc in REC.findall(sub_part)]
            if sub_recs and max(sc for _, sc in sub_recs) >= 0.5:
                fam = family_of(max(sub_recs, key=lambda r: r[1])[0])
            rows.append(dict(t=float(t), lx=float(lx), ly=float(ly), yaw=float(yaw),
                             fs=int(fs), v=float(v.group(1)) if v else 0.0,
                             sub=float(s.group(1)) if s else 0.0, fam=fam))
    return rows


def first_input(rows):
    for r in rows:
        if abs(r["lx"]) > 0.01 or abs(r["ly"]) > 0.01:
            return r["t"]
    return 0.0


def steps(pad_path):
    out = []
    with open(pad_path) as f:
        for line in f:
            line = line.split("#")[0].split()
            if len(line) >= 4:
                out.append((float(line[0]), float(line[1]), float(line[2]), float(line[3]),
                            line[4] if len(line) > 4 else ""))
    return out


def window(rows, t0, t1):
    return [r for r in rows if t0 <= r["t"] < t1]


def summarize(rows, t0, t1):
    w = window(rows, t0, t1)
    if not w:
        return None
    final = w[-1]["fam"]
    settle = next((r["t"] - t0 for r in w if r["fam"] == final), 0.0)
    flips = sum(1 for a, b in zip(w, w[1:]) if a["fam"] != b["fam"])
    tail = w[len(w) * 2 // 3:]
    v = sum(r["v"] for r in tail) / len(tail)
    sub = sum(r["sub"] for r in tail) / len(tail)
    yaw0, yaw1 = w[0]["yaw"], w[-1]["yaw"]
    return dict(fam=final, settle=settle, flips=flips, v=v, sub=sub, yaw0=yaw0, yaw1=yaw1)


def main():
    if len(sys.argv) != 4:
        print(__doc__)
        sys.exit(2)
    cpp, rust, pad = parse(sys.argv[1]), parse(sys.argv[2]), steps(sys.argv[3])
    if not cpp or not rust:
        print(f"no [gait] lines (cpp={len(cpp)} rust={len(rust)})")
        sys.exit(1)
    first_step = next(s for s in pad if abs(s[2]) > 0.01 or abs(s[3]) > 0.01)[0]
    oc, orust = first_input(cpp) - first_step, first_input(rust) - first_step
    print(f"aligned: cpp offset {oc:+.3f}s, rust offset {orust:+.3f}s\n")
    print(f"{'step':>5} {'stick':>12} {'btn':>4} | {'C++ fam':>9} {'settle':>6} {'flips':>5} {'v':>6} {'sub':>6} | {'Rust fam':>9} {'settle':>6} {'flips':>5} {'v':>6} {'sub':>6} | match")
    bad = 0
    for (t0, dur, lx, ly, btn) in pad:
        c = summarize(cpp, t0 + oc, t0 + dur + oc)
        r = summarize(rust, t0 + orust, t0 + dur + orust)
        if not c or not r:
            continue
        ok = c["fam"] == r["fam"] and abs(c["v"] - r["v"]) < 0.25 and abs(c["sub"] - r["sub"]) < 0.25
        bad += not ok
        print(f"{t0:5.1f} {f'({lx:+.1f},{ly:+.1f})':>12} {btn:>4} | "
              f"{c['fam']:>9} {c['settle']:6.2f} {c['flips']:5d} {c['v']:6.2f} {c['sub']:6.2f} | "
              f"{r['fam']:>9} {r['settle']:6.2f} {r['flips']:5d} {r['v']:6.2f} {r['sub']:6.2f} | "
              f"{'ok' if ok else 'DIFF'}")
    print(f"\n{bad} step(s) differ")


if __name__ == "__main__":
    main()
