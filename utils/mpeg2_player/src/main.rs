use anyhow::{bail, Context, Result};
use mpeg2_player::{DeinterlaceMode, Mpeg2Player, Mpeg2PlayerOptions};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let input = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!(
                "Usage: mpeg2_player <input.m2v|.mpg> [--dump-dir dir] [--rgba] [--deinterlace preserve|weave|bob]"
            );
            return Ok(());
        }
    };

    let mut dump_dir: Option<PathBuf> = None;
    let mut options = Mpeg2PlayerOptions::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump-dir" => {
                let dir = args.next().context("--dump-dir requires a directory")?;
                dump_dir = Some(PathBuf::from(dir));
            }
            "--rgba" => options.convert_to_rgba = true,
            "--deinterlace" => {
                let mode = args.next().context("--deinterlace requires a mode")?;
                options.deinterlace = DeinterlaceMode::parse(&mode)?;
            }
            other => bail!("unknown argument '{other}'"),
        }
    }

    let frames = Mpeg2Player::decode_file(&input, options.clone())?;
    println!(
        "Decoded {} frame buffer(s) from {}",
        frames.len(),
        input.display()
    );
    for (idx, frame) in frames.iter().enumerate() {
        println!(
            "#{idx:05}: {}x{} {:?} pts={:?} yuv420p={} bytes rgba={}",
            frame.width,
            frame.height,
            frame.field_order,
            frame.pts,
            frame.y.len() + frame.cb.len() + frame.cr.len(),
            frame.rgba.as_ref().map_or(0, Vec::len)
        );
    }

    if let Some(dir) = dump_dir {
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        for (idx, frame) in frames.iter().enumerate() {
            let yuv = dir.join(format!("frame-{idx:05}.yuv"));
            frame.write_yuv420p(&yuv)?;
            if options.convert_to_rgba {
                let rgba = dir.join(format!("frame-{idx:05}.rgba"));
                frame.write_rgba(&rgba)?;
            }
        }
        println!("Wrote decoded buffers to {}", dir.display());
    }

    Ok(())
}
