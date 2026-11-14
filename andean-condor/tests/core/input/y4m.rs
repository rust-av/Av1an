use super::*;

#[test]
fn y4m_header_length_is_one_extension_token() {
    let mut input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(
            "import vapoursynth as vs\n\
             vs.core.std.BlankClip(width=18, height=8, length=1, format=vs.YUV420P10, \
             fpsnum=30000, fpsden=1001).set_output()"
                .to_owned(),
        ),
        variables: HashMap::new(),
        index: 0,
        filters: Vec::new(),
        stream_concurrently: true,
    })
    .expect("y4m-length requires the installed native VapourSynth runtime");
    for (frames, extension) in [
        (None, None),
        (Some(0), Some("XLENGTH=0")),
        (Some(42), Some("XLENGTH=42")),
        (Some(4_294_967_295), Some("XLENGTH=4294967295")),
    ] {
        let header = input.y4m_header(frames).expect("actual decoder header");
        let tokens: Vec<_> = header.split_ascii_whitespace().collect();
        let mut expected = vec!["YUV4MPEG2", "C420p10", "W18", "H8", "F30000:1001", "Ip", "A0:0"];
        if let Some(extension) = extension {
            expected.push(extension);
        }
        assert_eq!(tokens, expected, "frames={frames:?}, header={header:?}");
        assert_eq!(
            tokens.iter().filter(|token| token.starts_with("XLENGTH")).count(),
            usize::from(frames.is_some()),
            "exactly one length extension when supplied"
        );
        assert!(header.ends_with('\n'));
        assert_eq!(header.lines().count(), 1);
    }
}

fn gray_roundtrip(bit_depth: usize, colors: [u16; 2], tag: &str, pixel_format: &str) {
    use std::{
        fs::{self, File},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let mut input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(format!(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.GRAY{bit_depth}, color=[{}])\n\
             b = c.std.BlankClip(a, color=[{}])\n(a + b).set_output()",colors[0],colors[1]
        )),
        variables: HashMap::new(), index: 0, filters: Vec::new(), stream_concurrently: true,
    }).expect("grayscale requires the installed native VapourSynth runtime");
    let details = input.decoder().get_video_details();
    assert_eq!(details.bit_depth, bit_depth);
    assert_eq!(details.chroma_sampling, ChromaSubsampling::Monochrome);
    let header = input.y4m_header(None).expect("actual decoder header");
    let mut stream = header.as_bytes().to_vec();
    for index in 0..2 {
        stream.extend(input.y4m_frame(index).expect("actual gray frame").into_inner());
    }
    let temp = tempfile::tempdir().expect("roundtrip directory");
    let source = temp.path().join("input.y4m");
    let decoded = temp.path().join("decoded.raw");
    let log = temp.path().join("ffmpeg.log");
    fs::write(&source, stream).expect("write actual header and production frames");
    // File-backed I/O cannot block on full pipes; bound and reap the child too.
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(&source)
        .args(["-map", "0:v:0", "-c:v", "rawvideo", "-pix_fmt", pixel_format, "-f", "rawvideo"])
        .arg(&decoded)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log).expect("ffmpeg log"))
        .spawn()
        .expect("grayscale requires ffmpeg on PATH");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll ffmpeg") {
            break status;
        }
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill timed-out ffmpeg");
            child.wait().expect("reap timed-out ffmpeg");
            panic!("GRAY{bit_depth}: ffmpeg timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "GRAY{bit_depth}, {header:?}: {}",
        fs::read_to_string(&log).expect("read ffmpeg log")
    );
    assert_eq!(header.split_ascii_whitespace().nth(1), Some(tag));
    let actual = fs::read(decoded).expect("decoded raw frames");
    let expected: Vec<u8> = colors
        .into_iter()
        .flat_map(|color| {
            if bit_depth == 8 {
                vec![u8::try_from(color).expect("8-bit sample"); 54]
            } else {
                color.to_le_bytes().repeat(54)
            }
        })
        .collect();
    assert_eq!(
        actual.len(),
        2 * 18 * 3 * if bit_depth == 8 { 1 } else { 2 },
        "exactly two complete frames"
    );
    assert_eq!(
        actual, expected,
        "GRAY{bit_depth}: exact sample values and frame alignment"
    );
}

#[test]
fn gray8_y4m_decodes_two_exact_frames_in_ffmpeg() {
    gray_roundtrip(8, [37, 219], "Cmono", "gray");
}

#[test]
fn gray10_y4m_decodes_two_exact_little_endian_frames() {
    gray_roundtrip(10, [0x0123, 0x03ab], "Cmono10", "gray10le");
}

#[test]
fn gray12_y4m_decodes_two_exact_little_endian_frames() {
    gray_roundtrip(12, [0x0234, 0x0bcd], "Cmono12", "gray12le");
}

#[test]
fn gray16_y4m_decodes_two_exact_little_endian_frames() {
    gray_roundtrip(16, [0x1234, 0xabcd], "Cmono16", "gray16le");
}
