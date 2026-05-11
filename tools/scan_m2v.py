"""Scan an MPEG-2 elementary stream and report the flags relevant for the
mpeg2_player_v2 crate's relaxed-guard subset.

Reports:
  - sequence info (size, frame rate, progressive_sequence)
  - per-picture: coding_type (I/P/B), picture_structure, frame_pred_frame_dct,
    progressive_frame, top_field_first
  - distribution counts so we know if ALL pictures meet the proposal's
    "frame_pred_frame_dct=1 frame picture" guard or only some.
"""

import sys
from collections import Counter


def find_start_codes(data: bytes):
    i = 0
    n = len(data)
    while i < n - 3:
        if data[i] == 0 and data[i + 1] == 0 and data[i + 2] == 1:
            yield i, data[i + 3]
            i += 4
        else:
            i += 1


class BitReader:
    def __init__(self, data: bytes, byte_offset: int):
        self.data = data
        self.bit_pos = byte_offset * 8

    def read(self, n: int) -> int:
        v = 0
        for _ in range(n):
            byte = self.data[self.bit_pos >> 3]
            bit = (byte >> (7 - (self.bit_pos & 7))) & 1
            v = (v << 1) | bit
            self.bit_pos += 1
        return v


def main(path: str) -> None:
    data = open(path, "rb").read()

    seq_size = None
    frame_rate_code = None
    progressive_sequence = None

    pic_coding_types = Counter()
    pic_structures = Counter()
    fpfd_values = Counter()
    progressive_frame_values = Counter()
    tff_values = Counter()
    pic_with_fpfd_zero_total = 0
    pictures_seen = 0

    last_picture_offset = None  # offset of last 0x00 picture_start_code

    for off, code in find_start_codes(data):
        if code == 0xB3:  # sequence header
            br = BitReader(data, off + 4)
            h = br.read(12)
            v = br.read(12)
            ar = br.read(4)
            fr = br.read(4)
            seq_size = (h, v, ar)
            frame_rate_code = fr
        elif code == 0xB5:  # extension start code
            ext_id = (data[off + 4] >> 4) & 0xF
            if ext_id == 0x1:  # sequence extension
                br = BitReader(data, off + 4)
                br.read(4)  # ext_id
                br.read(8)  # profile_and_level
                progressive_sequence = br.read(1)
            elif ext_id == 0x8:  # picture coding extension
                br = BitReader(data, off + 4)
                br.read(4)  # ext_id
                br.read(4)  # f_code[0][0]
                br.read(4)  # f_code[0][1]
                br.read(4)  # f_code[1][0]
                br.read(4)  # f_code[1][1]
                br.read(2)  # intra_dc_precision
                picture_structure = br.read(2)
                top_field_first = br.read(1)
                frame_pred_frame_dct = br.read(1)
                br.read(1)  # concealment_motion_vectors
                br.read(1)  # q_scale_type
                br.read(1)  # intra_vlc_format
                br.read(1)  # alternate_scan
                br.read(1)  # repeat_first_field
                br.read(1)  # chroma_420_type
                progressive_frame = br.read(1)

                pic_structures[picture_structure] += 1
                fpfd_values[frame_pred_frame_dct] += 1
                progressive_frame_values[progressive_frame] += 1
                tff_values[top_field_first] += 1
                if frame_pred_frame_dct == 0:
                    pic_with_fpfd_zero_total += 1
        elif code == 0x00:  # picture_start_code
            br = BitReader(data, off + 4)
            br.read(10)  # temporal_reference
            picture_coding_type = br.read(3)
            pic_coding_types[picture_coding_type] += 1
            pictures_seen += 1
            last_picture_offset = off

    print(f"=== {path} ({len(data):,} bytes) ===")
    if seq_size:
        h, v, ar = seq_size
        ar_names = {1: "1:1 SAR", 2: "4:3 DAR", 3: "16:9 DAR", 4: "2.21:1 DAR"}
        fr_table = {
            1: "23.976",
            2: "24",
            3: "25",
            4: "29.97",
            5: "30",
            6: "50",
            7: "59.94",
            8: "60",
        }
        print(f"  size: {h}x{v}")
        print(f"  aspect: {ar} ({ar_names.get(ar, '?')})")
        print(f"  frame rate code: {frame_rate_code} ({fr_table.get(frame_rate_code, '?')} fps)")
        print(f"  progressive_sequence: {progressive_sequence}")
    pct = {1: "I", 2: "P", 3: "B", 4: "D"}
    print(f"\n  pictures: {pictures_seen}")
    print(f"  picture_coding_type: {dict((pct.get(k, k), v) for k, v in pic_coding_types.items())}")
    ps_names = {1: "TOP_FIELD", 2: "BOTTOM_FIELD", 3: "FRAME"}
    print(f"  picture_structure: {dict((ps_names.get(k, k), v) for k, v in pic_structures.items())}")
    print(f"  frame_pred_frame_dct: {dict(fpfd_values)}")
    print(f"  progressive_frame: {dict(progressive_frame_values)}")
    print(f"  top_field_first: {dict(tff_values)}")

    print("\n--- proposal-fit ---")
    has_field_pic = (1 in pic_structures) or (2 in pic_structures)
    fpfd_one_count = fpfd_values.get(1, 0)
    fpfd_zero_count = fpfd_values.get(0, 0)
    if has_field_pic:
        print("  ✗ contains field pictures → REJECTED by proposal")
    else:
        print("  ✓ all pictures are frame pictures (proposal supports)")
    if fpfd_zero_count > 0 and fpfd_one_count == 0:
        print(f"  ✗ all {fpfd_zero_count} pictures have frame_pred_frame_dct=0 — encoder is allowed to use")
        print("    field-coded macroblocks; proposal rejects field-DCT/field-MC MBs.")
        print("    → Header-level guard would fail every picture in this file.")
    elif fpfd_zero_count > 0:
        print(f"  ⚠ {fpfd_zero_count}/{pictures_seen} have FPFD=0; {fpfd_one_count} have FPFD=1.")
        print("    Some pictures decode safely, others depend on macroblock content.")
    else:
        print(f"  ✓ all {fpfd_one_count} pictures have frame_pred_frame_dct=1 — proposal-safe.")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "rockstarlogo.m2v")
