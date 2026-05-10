# MPEG-2 video buffer player

`mpeg2_player` is a standalone, audio-free MPEG-2/H.262 video decoder utility.
It accepts MPEG-2 elementary streams (`.m2v`) and MPEG program streams (`.mpg`),
drops non-video PES payloads, decodes video to in-memory YUV420P frame buffers,
and can optionally dump those buffers as raw `.yuv` or `.rgba` files.

```bash
cargo run -p mpeg2_player -- path/to/video.m2v --dump-dir decoded --deinterlace bob --rgba
```

Interlaced frame-picture streams are supported when coded as 4:2:0 frame
pictures that use frame-MC/frame-DCT macroblocks. Streams with
`frame_pred_frame_dct = 0` are parsed for the per-macroblock
`frame_motion_type` and `dct_type` side bits so frame-mode macroblocks stay
aligned; field pictures and macroblocks that actually request field-DCT,
field-MC, or dual-prime prediction are rejected with explicit errors instead
of silently returning corrupt frames.
