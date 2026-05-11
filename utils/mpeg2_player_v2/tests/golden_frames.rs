use mpeg2_player_v2::{FramePlanes, Mpeg2Player};
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
#[ignore = "milestone 4 gate: enable once I-frame slice/macroblock decode is wired into next_frame"]
fn generated_progressive_simple_matches_ffmpeg_yuv() {
    let temp = std::env::temp_dir().join(format!(
        "mpeg2_player_v2_golden_{}_{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&temp).unwrap();
    let fixture = temp.join("progressive_simple.m2v");
    generate_progressive_simple_fixture(&fixture);

    let reference = temp.join("ffmpeg.yuv");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            fixture.to_str().unwrap(),
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "-y",
            reference.to_str().unwrap(),
        ])
        .output()
        .expect("ffmpeg should be installed for golden-frame tests");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let expected = fs::read(&reference).unwrap();
    let mut player = Mpeg2Player::open(&fixture).unwrap();
    assert_eq!(player.info().width, 16);
    assert_eq!(player.info().height, 16);
    assert!(player.info().progressive_sequence);

    let frame = player
        .next_frame()
        .expect("decoder should produce the first I frame")
        .expect("fixture should contain one decoded frame");
    let FramePlanes::Yuv420p { y, cb, cr } = frame.planes else {
        panic!("expected YUV420P output");
    };
    let mut actual = y;
    actual.extend_from_slice(&cb);
    actual.extend_from_slice(&cr);
    assert_eq!(actual.len(), expected.len());
    for (idx, (&got, &want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            got.abs_diff(want) <= 1,
            "byte {idx} differs by more than ±1 LSB: got {got}, expected {want}"
        );
    }
    fs::remove_dir_all(temp).ok();
}

fn generate_progressive_simple_fixture(path: &Path) {
    let output = Command::new("ffmpeg")
        .args([
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
            path.to_str().unwrap(),
        ])
        .output()
        .expect("ffmpeg should be installed for golden-frame tests");
    assert!(
        output.status.success(),
        "ffmpeg fixture generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
