# MPEG-2 video buffer player

`mpeg2_player` is a standalone, audio-free MPEG-2/H.262 video decoder utility.
It accepts MPEG-2 elementary streams (`.m2v`) and MPEG program streams (`.mpg`),
drops non-video PES payloads, decodes video to in-memory YUV420P frame buffers,
and can optionally dump those buffers as raw `.yuv` or `.rgba` files.

```bash
cargo run -p mpeg2_player -- path/to/video.m2v --dump-dir decoded --deinterlace bob --rgba
```

Interlaced frame-picture streams are supported when coded as 4:2:0 frame
pictures with frame-prediction/frame-DCT. Field pictures and field-DCT/field-MC
macroblocks are detected and rejected with explicit errors instead of silently
returning corrupt frames.
