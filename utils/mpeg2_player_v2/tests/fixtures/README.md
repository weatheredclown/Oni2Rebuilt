# MPEG-2 golden-frame fixtures

Binary MPEG-2 fixtures are intentionally **not** committed to this repository.
Use the harness in `../../scripts/compare_ffmpeg_yuv.py` to generate the tiny
`progressive_simple.m2v` fixture in a temporary directory when ffmpeg is
available.

The generated first target is:

- dimensions: 16×16
- chroma: 4:2:0 (`yuv420p`)
- picture structure: progressive frame picture
- picture content: one synthetic `testsrc2` frame

Generation command used by the harness:

```sh
ffmpeg -f lavfi -i testsrc2=size=16x16:rate=1:duration=1 \
  -frames:v 1 -pix_fmt yuv420p -c:v mpeg2video -g 1 -bf 0 -q:v 2 \
  -f mpeg2video progressive_simple.m2v
```

The harness also generates a temporary ffmpeg YUV420P decode and compares the
player output with ±1 LSB tolerance.
