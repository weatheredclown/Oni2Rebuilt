use mpeg2_player_v2::{
    ChromaUpsample, DeinterlaceMode, Error, FramePlanes, Mpeg2Player, colorspace,
};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let input = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!(
                "Usage: mpeg2_player_v2 <input.m2v|.mpg> [--dump-dir dir] [--rgba] [--deinterlace preserve|weave|bob]"
            );
            return Ok(());
        }
    };

    let mut dump_dir = None;
    let mut rgba = false;
    let mut deinterlace = DeinterlaceMode::Preserve;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump-dir" | "--dump" => {
                dump_dir = Some(PathBuf::from(
                    args.next().ok_or("--dump-dir requires a directory")?,
                ))
            }
            "--rgba" => rgba = true,
            "--deinterlace" => {
                let value = args
                    .next()
                    .ok_or("--deinterlace requires preserve, weave, or bob")?;
                deinterlace = DeinterlaceMode::parse(&value).ok_or("unknown deinterlace mode")?;
            }
            other => return Err(format!("unknown argument '{other}'").into()),
        }
    }

    let mut player = Mpeg2Player::open(&input)?;
    player.set_deinterlace(deinterlace);
    let info = player.info();
    println!(
        "{}: {}x{} {}/{} fps {:?} progressive_sequence={}",
        input.display(),
        info.width,
        info.height,
        info.frame_rate_num,
        info.frame_rate_den,
        info.aspect_ratio,
        info.progressive_sequence
    );

    if let Some(dir) = &dump_dir {
        fs::create_dir_all(dir)?;
    }

    let mut count = 0usize;
    loop {
        match player.next_frame() {
            Ok(Some(frame)) => {
                if let Some(dir) = &dump_dir {
                    match frame.planes {
                        FramePlanes::Yuv420p { y, cb, cr } => {
                            let mut out = Vec::with_capacity(y.len() + cb.len() + cr.len());
                            out.extend_from_slice(&y);
                            out.extend_from_slice(&cb);
                            out.extend_from_slice(&cr);
                            fs::write(dir.join(format!("frame-{count:05}.yuv")), out)?;
                            if rgba {
                                let rgba = colorspace::yuv420p_to_rgba8(
                                    frame.width as usize,
                                    frame.height as usize,
                                    &y,
                                    &cb,
                                    &cr,
                                    ChromaUpsample::Bilinear,
                                );
                                fs::write(dir.join(format!("frame-{count:05}.rgba")), rgba)?;
                            }
                        }
                        FramePlanes::Rgba8(bytes) => {
                            fs::write(dir.join(format!("frame-{count:05}.rgba")), bytes)?
                        }
                    }
                }
                count += 1;
            }
            Ok(None) => break,
            Err(Error::DecodeNotImplemented(message)) => {
                eprintln!("Decode stopped: {message}");
                break;
            }
            Err(err) => return Err(err.into()),
        }
    }
    println!("Decoded {count} frame(s)");
    Ok(())
}
