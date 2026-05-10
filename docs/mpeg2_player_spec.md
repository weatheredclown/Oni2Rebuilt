# Pure-Rust MPEG-2 Player — From-Scratch Spec

## Why this exists

`utils/mpeg2_player` currently wraps a vendored decoder (`oxideav-mpeg12video`)
that was a "first-pass" subset codec — it explicitly rejects features that
2000s-era game cinematics actually use. Specifically, Oni2 ships
`rockstarlogo.m2v` (and likely other clips) encoded as **interlaced frame
pictures with `frame_pred_frame_dct = 0`, `intra_vlc_format = 1`,
`alternate_scan = 1`, plus per-MB field-DCT and field-MC**. Extending the
vendored crate piecemeal is one path; this spec describes the alternative —
a focused MPEG-2 video player implemented from scratch, scoped to the
content the game actually uses, with no dependency on oxideav.

The reference target is a 512×448 29.97 fps NTSC interlaced clip; if the
implementation plays that file and produces frames pixel-equivalent (within
±1 LSB rounding tolerance) to ffmpeg's output, it is correct.

## Scope

### In-scope (must work)

- **MPEG-2 video** (ISO/IEC 13818-2, "H.262"), Main Profile @ Main Level
- **4:2:0 chroma** only
- **Frame pictures only** (`picture_structure = 11`); both progressive and
  interlaced sequences (`progressive_sequence = 0` or `1`)
- All combinations of `frame_pred_frame_dct ∈ {0, 1}`,
  `intra_vlc_format ∈ {0, 1}`, `alternate_scan ∈ {0, 1}`,
  `q_scale_type ∈ {0, 1}`, `intra_dc_precision ∈ {0, 1, 2}` (8/9/10 bit)
- **I, P, B picture types** with full forward + backward + bidirectional MC
- Per-MB **field-DCT** (`dct_type = 1`) and **field-MC**
  (`frame_motion_type = 0b01`) within frame pictures
- Both **MPEG-1 elementary streams** (`.m2v` raw) and **MPEG-2 program
  streams** (`.mpg` with PES wrapping) as input — demuxer extracts video
  PES payload only, audio is discarded
- **Deinterlacing**: `preserve` (raw frame), `weave` (interlaced frame as
  one image), `bob` (emit two field-height-doubled frames per coded frame)
- Output buffers in **YUV420P** (planar Y/Cb/Cr) and **RGBA8** (BT.601
  limited-range conversion)
- Public Rust API + a small CLI

### Out-of-scope (reject with explicit error)

- **Field pictures** (`picture_structure ∈ {01, 10}`) — the macroblock
  addressing path differs and game content doesn't use them
- **Dual-prime motion compensation** (`frame_motion_type = 0b11`) — almost
  never used outside of broadcast content
- 4:2:2 / 4:4:4 chroma (`chroma_format ≠ 0b01`)
- Concealment motion vectors
- 16×8 motion vectors (which only appear in field pictures anyway)
- Audio decoding of any kind (MP2, AC-3, LPCM)
- Encoding — this is a player, not a codec
- Hardware acceleration (NVDEC / DXVA / VAAPI)
- Streaming / network input — file paths only

## Architecture

```
                      ┌────────────────────┐
   .m2v / .mpg ──────►│  demux (optional)  │── elementary stream bytes
                      └────────────────────┘
                                │
                                ▼
                      ┌────────────────────┐
                      │  bitstream reader  │── start-code scan, BitReader
                      └────────────────────┘
                                │
                                ▼
              ┌─────────────────┴──────────────────┐
              ▼                                    ▼
   ┌───────────────────┐              ┌───────────────────────┐
   │ headers (seq/gop/ │              │  slice loop           │
   │  pic + extensions)│              │   macroblock loop     │
   └───────────────────┘              │     ├ MB modes         │
                                      │     ├ MV decode        │
                                      │     ├ block decode     │
                                      │     │  (VLC → dequant  │
                                      │     │   → inverse-zig  │
                                      │     │   → IDCT)        │
                                      │     └ MC + add residue │
                                      └───────────────────────┘
                                                │
                                                ▼
                                      ┌───────────────────────┐
                                      │  picture buffer       │
                                      │   (past, future, cur) │
                                      └───────────────────────┘
                                                │
                                                ▼
                                      ┌───────────────────────┐
                                      │ deinterlace + convert │
                                      │   (yuv420p / rgba)    │
                                      └───────────────────────┘
                                                │
                                                ▼
                                          VideoFrame iterator
```

## Crate layout (proposed)

Single workspace crate `utils/mpeg2_player_v2/` with:

```
src/
├── lib.rs              public API + Player struct
├── main.rs             CLI
├── bitstream.rs        BitReader, start-code scanner
├── demux.rs            PES/PS demux for .mpg input
├── headers.rs          sequence/extension/GOP/picture/slice header parsing
├── picture_params.rs   PictureParams struct (resolved per-picture config)
├── slice.rs            decode_slice loop
├── mb.rs               macroblock decode (modes, MVs, blocks)
├── motion.rs           forward/backward/field MC predictors
├── block.rs            VLC → coefficient decode + IDCT entry
├── dequant.rs          intra & non-intra dequantisation, q-matrix handling
├── idct.rs             8×8 IDCT (libavcodec-style integer impl is fine)
├── scan.rs             zigzag + alternate scan permutation tables
├── colorspace.rs       YUV420P → RGBA8 conversion (BT.601 limited)
├── deinterlace.rs      weave / bob / preserve modes
├── tables/
│   ├── mod.rs
│   ├── mba.rs          Table B.1
│   ├── mb_type_i.rs    Table B.2
│   ├── mb_type_p.rs    Table B.3
│   ├── mb_type_b.rs    Table B.4
│   ├── cbp.rs          Table B.9
│   ├── motion.rs       Table B.10
│   ├── dct_dc_size.rs  Tables B.12, B.13
│   ├── dct_coeffs.rs   Tables B.14, B.15
│   └── q_matrices.rs   default intra + non-intra 8×8 quant matrices
└── error.rs            Error / Result
```

No dependencies beyond `std` and (optionally) a small bit-reader crate.

## Public API (target)

```rust
pub struct Mpeg2Player { /* ... */ }

impl Mpeg2Player {
    /// Open a .m2v or .mpg from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;

    /// Open from in-memory bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self>;

    /// Sequence-level metadata, available as soon as `open` returns.
    pub fn info(&self) -> &SequenceInfo;

    /// Pull the next decoded frame.  Returns `None` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>>;

    /// Configure deinterlacing.  Default: `Preserve`.
    pub fn set_deinterlace(&mut self, mode: DeinterlaceMode);
}

pub struct SequenceInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub aspect_ratio: AspectRatio,
    pub progressive_sequence: bool,
}

pub enum DeinterlaceMode { Preserve, Weave, Bob }

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub planes: FramePlanes,
    pub picture_type: PictureType,
    pub interlaced: bool,
    pub top_field_first: bool,
    pub presentation_time: f64, // seconds since stream start
}

pub enum FramePlanes {
    Yuv420p { y: Vec<u8>, cb: Vec<u8>, cr: Vec<u8> },
    Rgba8(Vec<u8>),
}
```

## Algorithms — required behavior

This section enumerates what each module must implement. The ISO/IEC
13818-2 spec is the source of truth; references like "§7.4.2" point to that
document. libavcodec's `mpegvideo_dec.c` and ffmpeg's `mpeg12dec.c` are the
recommended cross-reference implementations.

### Bitstream + start-code scan
- Standard byte-aligned start codes: `0x00 0x00 0x01 <code>`
- Codes used by this scope: `0x00` (picture), `0x01..0xAF` (slice),
  `0xB2` (user data, skip), `0xB3` (sequence header), `0xB5` (extension),
  `0xB7` (sequence end), `0xB8` (GOP), and `0xBA..0xBF` (PES, only in `.mpg`)
- BitReader supports `read(n: u8) -> u32` for `n ≤ 32` and `align_to_byte()`.

### Demux (`.mpg` only)
- Read pack header (`0xBA`), system header (`0xBB` — skip)
- For each PES packet (`0xE0..0xEF` = video stream IDs), strip the PES
  header (variable length per §2.4.3) and append payload to the
  elementary-stream buffer
- Discard audio (`0xC0..0xDF`) and other (`0xBD`) PES streams

### Headers
- Sequence header (§6.2.2.1) → width, height, aspect, frame rate,
  bit_rate, vbv_buffer_size, optional intra/non-intra quantizer matrices
- Sequence extension (§6.2.2.3) → profile/level, progressive_sequence,
  chroma_format, frame_rate_extension
- Sequence display extension (skip, but consume bits)
- GOP header (§6.2.2.6) → drop_frame, hours/minutes/seconds/frames (advisory)
- Picture header (§6.2.3) → temporal_reference, picture_coding_type,
  vbv_delay, full_pel_*_vector (MPEG-1), forward/backward_f_code (MPEG-1)
- Picture coding extension (§6.2.3.1) → f_code[0..1][0..1],
  intra_dc_precision, picture_structure, top_field_first,
  frame_pred_frame_dct, concealment_motion_vectors, q_scale_type,
  intra_vlc_format, alternate_scan, repeat_first_field, chroma_420_type,
  progressive_frame, composite_display_flag

### Slice loop (§6.2.4)
- Slice header: quantiser_scale_code (5 bits), optional intra_slice_flag,
  optional extra slice information bits
- Loop:
  1. Decode macroblock_address_increment (Table B.1) — skipped MBs in P/B
     pictures predict from the past reference with zero MV
  2. Decode macroblock_type (Tables B.2/B.3/B.4 by picture type) →
     intra/pattern/motion_forward/motion_backward/quant flags
  3. If `quant`: read 5-bit quantiser_scale_code
  4. **If `frame_pred_frame_dct == 0`** and the MB has motion: read 2-bit
     `frame_motion_type` → require `0b10` (frame MC). `0b01` = field MC
     (must be supported). `0b11` = dual-prime (reject as unsupported).
  5. Motion vector decode (one MV pair per direction for frame MC; two pairs
     plus `motion_vertical_field_select` bits for field MC)
  6. **If `frame_pred_frame_dct == 0`** and (pattern || intra): read 1-bit
     `dct_type`. 0 = frame DCT, 1 = field DCT.
  7. CBP (Table B.9) for non-intra MBs
  8. For each coded 8×8 block: VLC decode → inverse-zigzag → dequantise
     → IDCT
  9. Predict via MC (or use intra DC predictor for I MBs); add residue
- End-of-slice when next 23 bits are zero (start-code prefix found)

### Field DCT (`dct_type = 1`)
The four luma blocks of a field-DCT MB are organised differently:
- Block 0: rows 0,2,4,6,8,10,12,14 (top field, left half)
- Block 1: rows 0,2,4,6,8,10,12,14 (top field, right half)
- Block 2: rows 1,3,5,7,9,11,13,15 (bottom field, left half)
- Block 3: rows 1,3,5,7,9,11,13,15 (bottom field, right half)

Decode and IDCT each block normally; on write-back to the picture, scatter
rows back to spatial order. The chroma blocks are unaffected (still
frame-organised in 4:2:0). See §6.1.3 + Annex A.

### Field MC (`frame_motion_type = 0b01`)
Two MVs per direction; each predicts an 8-row half of the MB:
- For each direction (forward and/or backward):
  - Read MV pair 1 + 1-bit `motion_vertical_field_select_1`
  - Read MV pair 2 + 1-bit `motion_vertical_field_select_2`
- The first MV predicts the top-field rows (0,2,4,...,14) of the MB from
  the field selected by bit 1; the second MV predicts the bottom-field rows
  (1,3,...,15) from the field selected by bit 2
- MV vertical components are **measured in field-line units** but the
  prediction reads from the chosen reference field's frame buffer offset
  appropriately (see §7.6.4)
- Chroma: half-vertical-MV applies; same field-select per direction

### Inverse zigzag
- Default scan: standard zigzag (Table 7-3 in §7.6 / Annex A)
- Alternate scan when `alternate_scan = 1`: see Table 7-4. This is a
  different 64-element permutation, also documented as the "vertical scan"
  designed for interlaced content where vertical correlation differs.

### Dequantisation
- Intra: `f[v][u] = (2 * QF[v][u] + k) * W[w][v][u] * quantiser_scale / 32`,
  with `k = 0` for intra (coefficient sign already in `QF`), and `W` taken
  from the appropriate quantiser matrix
- Non-intra: similar, k = sign(QF) (see §7.4.2.3)
- `quantiser_scale` derived from `quantiser_scale_code` per §7.4.2.2 — the
  table differs between `q_scale_type = 0` and `q_scale_type = 1`
- Saturate result to `[-2048, 2047]` (signed 12-bit)
- Mismatch control: after all 64 coefficients dequantised, XOR the LSB of
  every coefficient; if the running sum is even, toggle bit 0 of `f[7][7]`
  (§7.4.4)

### IDCT
- Any conformant 8×8 IDCT works. Reference implementations: the integer
  IDCT from libavcodec (`ff_simple_idct_int16_8bit`) or the AAN IDCT.
- IEEE 1180-1990 conformance is preferred but not required for visual
  correctness.

### DCT-DC prediction (intra)
- Per §7.2.1: separate Y / Cb / Cr predictors, reset at slice start, MB
  boundary in some cases, and after non-intra MBs in a slice
- DC value derived from `dct_dc_size_*` VLC (Tables B.12 / B.13) plus
  `intra_dc_precision`-bit raw differential

### `intra_vlc_format` (Table B.15)
- When `intra_vlc_format = 1`, intra-coded blocks use Table B.15 instead of
  Table B.14 for run/level decode after the DC coefficient
- Same RUN/LEVEL semantics, different VLC codes — straight table swap

### Motion compensation
- Half-pel interpolation: bilinear (sum + 1) >> 1 for half-positions, plain
  copy for integer positions, on each reference pixel
- Bidirectional prediction: forward + backward halves, average ((a+b+1)>>1)
- Boundary clamping: clamp MV-derived reference coordinates to the picture
  rectangle (see §7.6.3)

### Picture buffering and output ordering
- Maintain three slots: `past_ref`, `future_ref`, `current`
- Pictures decode in bitstream order; B pictures emit immediately, I/P
  pictures hold for one frame so the following B can reference both. Output
  order = display order, derived from `temporal_reference`.

### Deinterlacing (post-decode)
- `Preserve`: emit the decoded frame buffer untouched
- `Weave`: same as Preserve but flag `interlaced = true`
- `Bob`: for an interlaced frame, emit two output frames — one from the
  top-field rows scaled vertically (×2 by line doubling), one from the
  bottom field. Use linear interpolation between adjacent lines for the
  doubled rows. Each output is full-height.

### Colorspace conversion (when caller requests `Rgba8`)
- BT.601 limited-range (Y in 16..=235, Cb/Cr in 16..=240)
- 4:2:0 chroma upsample: nearest-neighbour or bilinear (caller's choice;
  default bilinear)

## Test plan

### Unit tests (per module, no external deps)
- Bitstream: read random bit lengths, alignment, start-code scanner
- Headers: round-trip a hand-built sequence header
- VLC tables: every (code, bits) pair decodes correctly + no prefix
  collisions (existing oxideav has a good pattern for this)
- IDCT: known input/output pairs from IEEE 1180-1990 reference vectors
- Dequant: manually compute one block against the reference formula
- MV prediction: hand-built test cases with known reference pictures

### Integration tests (golden frames)
Required infrastructure:
1. A small test fixtures directory with **3–5 sample MPEG-2 files** of
   varying difficulty:
   - `progressive_simple.m2v` — straight I-frames only, FPFD=1
   - `progressive_pb.m2v` — I/P/B with frame MC, FPFD=1
   - `interlaced_frame_dct.m2v` — interlaced, FPFD=0, only frame DCT MBs
   - `interlaced_field_dct.m2v` — same plus field-DCT MBs
   - `rockstarlogo.m2v` — the actual target file
2. **Golden frames** decoded by `ffmpeg -i $FIXTURE -pix_fmt yuv420p
   $FIXTURE_%04d.yuv`, committed alongside the fixtures (or generated by a
   script in CI from a hash-pinned ffmpeg build)
3. A test that runs the player on each fixture and asserts every output
   plane matches the corresponding ffmpeg golden within ±1 LSB on every
   pixel (the ±1 tolerance accounts for IDCT rounding differences)

This is the only way to get the field-MC code path correct — the bug
surface is too large to validate by inspection.

### Performance targets (informational, not blocking)
- Decode 512×448 @ 30 fps in real-time on a single thread on modern
  desktop hardware (≪ 33 ms per frame)
- No allocations per macroblock — pre-allocate scratch buffers in `Player`

## Implementation milestones

The order minimises rework — earlier milestones are dependencies of later
ones. Each milestone has a clear pass/fail signal.

| # | Milestone                                            | Pass signal                              |
|---|------------------------------------------------------|------------------------------------------|
| 1 | Bitstream reader + start-code scanner                | Tokenises a real .m2v into start codes   |
| 2 | Sequence/picture/extension header parsing            | Round-trip test passes                   |
| 3 | All VLC tables (B.1–B.15) wired with no collisions   | No-collision property test passes        |
| 4 | I-frame decode (progressive, FPFD=1, IVF=0, AS=0)    | `progressive_simple.m2v` matches golden  |
| 5 | P/B frame decode + frame-MC                          | `progressive_pb.m2v` matches golden      |
| 6 | `intra_vlc_format=1` and `alternate_scan=1`          | Adds two test fixtures, both match       |
| 7 | Field-DCT macroblocks                                | `interlaced_field_dct.m2v` matches       |
| 8 | Field-MC macroblocks                                 | `rockstarlogo.m2v` matches golden        |
| 9 | Deinterlace modes                                    | Visual sanity check (no test for this)   |
| 10| `.mpg` PES demux                                     | A PS-wrapped fixture decodes identically |
| 11| RGBA conversion                                      | A canned YUV→RGBA conversion test passes |
| 12| Public API + CLI                                     | `mpeg2_player_v2 --dump rockstarlogo.m2v`|

## Reference material

- **ISO/IEC 13818-2** ("H.262") — the spec itself. The 2000 edition is
  freely available from ITU as ITU-T Rec. H.262.
- **libavcodec** (`mpeg12dec.c`, `mpegvideo_dec.c`) — battle-tested
  reference implementation. Apache-/LGPL-licensed; do not copy code, but
  consult for table values and edge-case handling.
- **mpeg2dec / libmpeg2** — older, simpler reference. GPL-licensed.
- **The MPEG Handbook** (Watkinson) — readable narrative explanation,
  good companion to the spec.

## What "done" looks like

- `cargo run -p mpeg2_player_v2 -- rockstarlogo.m2v --dump-dir out/` writes
  266 PNGs (or YUV blobs) that visually match ffmpeg's decode and pass the
  ±1 LSB golden test
- `cargo test -p mpeg2_player_v2` is green
- The crate has zero non-std runtime deps
- Demux works for at least one `.mpg` file from the game (or a synthetic
  one) — even if `rockstarlogo.m2v` itself doesn't exercise that path

## Notes for whoever picks this up

- Don't try to implement everything in one pass. Milestones 1–4 give you a
  working I-frame decoder; once that's matching ffmpeg on a simple fixture
  you'll have caught most bitstream-reader bugs and the IDCT will be
  validated. After that, each subsequent milestone builds on a known-good
  foundation.
- The single highest-risk milestone is #8 (field-MC). Save 30–50% of total
  budget for it. The bug categories: wrong field selected for prediction,
  wrong vertical offset into the reference, off-by-one row interleaving on
  output, sign errors in MV reconstruction.
- Don't bother implementing dual-prime — it's gated as Unsupported and
  game content essentially never uses it.
- The repo already has `tools/scan_m2v.py` which dumps per-picture flags;
  use it to characterise any new fixture before debugging decode failures.
