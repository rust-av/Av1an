use crate::core::encoder::parse::*;

#[test]
fn malformed_counts() {
    for count in ["+1", "-1", "", "1x", "1.0", "１", "18446744073709551616"] {
        let line = format!("Pass 1/1 frame    1/{count} 12345678901234567890B");
        assert_eq!(parse_aom_vpx_frames(&line), None, "count: {count:?}");
    }
}

fn assert_parser_parity(input: &[u8], expected: Option<u64>) {
    use crate::core::encoder::Encoder;

    if let Ok(line) = std::str::from_utf8(input) {
        assert_eq!(parse_aom_vpx_frames(line), expected, "scalar: {input:?}");
        for encoder in [EncoderBase::AOM, EncoderBase::VPX, EncoderBase::AVM] {
            assert_eq!(
                Encoder::parse_encoded_frames_base(&encoder, line),
                expected,
                "public {encoder:?}: {input:?}"
            );
        }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
        assert_eq!(
            // SAFETY: Both required CPU features are checked. The parser must
            // bound every load, including malformed and truncated input.
            unsafe { parse_aom_vpx_frames_sse41(input) },
            expected,
            "SIMD: {input:?}"
        );
    }
}

// Removing a delimiter/range/digit check, truncating wide counts, or dropping the
// scalar fallback must break this corpus, independently of parser-derived values.
fn guard_page_inputs(mut check: impl FnMut(&[u8], Option<u64>)) -> usize {
    let mut cases = 0;
    let mut check = |input: &[u8], expected| {
        cases += 1;
        check(input, expected);
    };
    for count in [
        0,
        1,
        9,
        12,
        123,
        1234,
        12345,
        123_456,
        1_234_567,
        12_345_678,
        123_456_789,
        1_234_567_890,
        12_345_678_901,
        123_456_789_012,
        1_234_567_890_123,
        12_345_678_901_234,
        123_456_789_012_345,
        999_999_999_999_999,
        1_234_567_890_123_456,
        9_999_999_999_999_999,
        12_345_678_901_234_567,
        123_456_789_012_345_678,
        1_234_567_890_123_456_789,
        12_345_678_901_234_567_890,
        u64::MAX,
    ] {
        for slash in 19..=53 {
            let mut line = vec![b'x'; slash];
            line.extend_from_slice(format!("/{count} ").as_bytes());
            let terminated = line.len();
            line.extend_from_slice(b"123456789012345678901234567890123B");
            for end in 0..=line.len() {
                check(&line[..end], (end >= terminated).then_some(count));
            }
        }
    }
    for count in [
        "",
        "+1",
        "-1",
        "1x",
        "x1",
        "1.0",
        "1/2",
        "1\t",
        "１",
        "\u{a0}1",
        "18446744073709551616",
        "99999999999999999999",
        "99999999999999999999999999999999",
    ] {
        for slash in 19..=53 {
            let mut line = vec![b'x'; slash];
            line.extend_from_slice(format!("/{count} ").as_bytes());
            check(&line, None);
            line.extend_from_slice(b"123456789012345678901234567890123B");
            check(&line, None);
        }
    }
    for count in ["00000000000000000000000000000000", "00000000000000000000000000000001"] {
        let line = format!("Pass 1/1 frame    1/{count} ");
        check(line.as_bytes(), Some(u64::from(count.ends_with('1'))));
    }
    for len in 0..=96 {
        let mut line = vec![b'7'; len];
        check(&line, None); // Neither required separator.
        if let Some(last) = line.last_mut() {
            *last = b' ';
            check(&line, None); // No slash.
        }
        for slash in 19..len {
            let mut line = vec![b'7'; len];
            line[slash] = b'/';
            check(&line, None); // No space, including all 16-byte boundaries.
        }
    }
    for byte in 0..=u8::MAX {
        let mut line = b"Pass 1/1 frame    1/0 12345678901234567890B".to_vec();
        line[20] = byte;
        let expected = byte.is_ascii_digit().then(|| u64::from(byte - b'0'));
        check(&line, expected);
    }
    for place in 0..15 {
        for byte in 0..=u8::MAX {
            let mut line = b"Pass 1/1 frame    1/000000000000000 12345678901234567890B".to_vec();
            line[20 + place] = byte;
            // A space after a zero is a valid earlier terminator; trailing
            // progress fields are deliberately outside this parser's grammar.
            let expected = if byte == b' ' && place > 0 {
                Some(0)
            } else {
                byte.is_ascii_digit()
                    .then(|| u64::from(byte - b'0') * 10_u64.pow(14 - place as u32))
            };
            check(&line, expected);
        }
    }
    cases
}

#[test]
fn prefix_truncation_matrix() {
    isolate_guard_page_case(
        "core::encoder::parse::tests::prefix_truncation_matrix",
        || {
            let cases = guard_page_inputs(assert_parser_parity);
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            println!(
                "parser-bounds matrix: {cases} cases; SSSE3+SSE4.1={}",
                is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3")
            );
            #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
            println!("parser-bounds matrix: {cases} cases; SIMD unavailable on this architecture");
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn guard_page_bounds() {
    isolate_guard_page_case(
        "core::encoder::parse::tests::guard_page_bounds",
        guard_page_cases,
    );
}

fn isolate_guard_page_case(test_name: &str, run: impl FnOnce()) {
    use std::{
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    const CHILD: &str = "AV1AN_PARSER_BOUNDS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        #[cfg(target_os = "linux")]
        assert_eq!(
            // SAFETY: Only this child disables core dumps so a regression cannot
            // leave a core file behind; the limit struct is valid for the call.
            unsafe {
                libc::setrlimit(
                    libc::RLIMIT_CORE,
                    &libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    },
                )
            },
            0
        );
        run();
        println!("parser-bounds child cases completed");
        return;
    }

    // Re-exec rather than fork a multithreaded libtest process. Any old-parser
    // overread faults only in this bounded child, never in the parent test.
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start parser child");
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().expect("poll parser child").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out parser child");
            child.wait().expect("reap timed-out parser child");
            panic!("parser child exceeded 30 seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("parser child output");
    assert!(
        output.status.success(),
        "parser child {test_name}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("parser-bounds child cases completed")
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

#[cfg(target_os = "linux")]
fn guard_page_cases() {
    // SAFETY: sysconf has no pointer arguments or additional preconditions.
    let page_size =
        usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).expect("positive page size");
    // SAFETY: Request a fresh anonymous two-page mapping, with no fixed address.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_size * 2,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    let bytes = mapping.cast::<u8>();
    assert_eq!(
        // SAFETY: The second page is page-aligned and entirely inside this mapping.
        unsafe { libc::mprotect(bytes.add(page_size).cast(), page_size, libc::PROT_NONE) },
        0
    );

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
        eprintln!("parser-bounds SSSE3+SSE4.1 active in isolated child");
    } else {
        eprintln!(
            "parser-bounds SIMD skipped: SSSE3+SSE4.1 unavailable; scalar/public still checked"
        );
    }
    // Each slice ends exactly at PROT_NONE. Varying lengths also varies the start
    // alignment; neither spare allocation capacity nor malloc padding can hide a load.
    let cases = guard_page_inputs(|source, expected| {
        assert!(source.len() <= page_size);
        // SAFETY: The copy fits in the first mapped page, does not overlap its
        // source, and the immutable slice lasts only until the next copy.
        let input = unsafe {
            let start = bytes.add(page_size - source.len());
            std::ptr::copy_nonoverlapping(source.as_ptr(), start, source.len());
            std::slice::from_raw_parts(start, source.len())
        };
        assert_parser_parity(input, expected);
    });
    // SAFETY: This is the complete still-live mapping returned by mmap above.
    assert_eq!(unsafe { libc::munmap(mapping, page_size * 2) }, 0);
    println!("parser-bounds guard-page cases completed: {cases}");
}

#[test]
fn vvenc_keyed_stats_reach_public_parser_and_dispatcher() {
    use crate::core::encoder::Encoder;

    // Stats.h at a455d51e335c97b40eeea7bb90e3ad52b84c6e83 emits both forms.
    for (line, expected) in [
        (
            "vvenc [info]: stats:  frame= 60 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 0",
            60,
        ),
        (
            "vvenc [info]: stats: 50.0%% frame= 60/120 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 0",
            60,
        ),
        (
            "vvenc [info]: stats:\t fps= 900 elapsed= 1\tframe=\t60/120\tavg_fps= 901\r\n",
            60,
        ),
        ("vvenc [info]: stats: frame=60 fps= 900", 60),
        ("vvenc [info]: stats: fps= 900 frame=60/120", 60),
        ("vvenc [info]: stats: frame=0", 0),
        ("vvenc [info]: stats: frame=60/0", 60),
        ("vvenc [info]: stats: frame=60/1", 60),
        (
            "vvenc [info]: stats: frame=18446744073709551615/18446744073709551615",
            u64::MAX,
        ),
    ] {
        assert_eq!(parse_vvc_frames(line), Some(expected), "parser: {line:?}");
        assert_eq!(
            Encoder::parse_encoded_frames_base(&EncoderBase::VVenC, line),
            Some(expected),
            "dispatcher: {line:?}"
        );
    }
}

#[test]
fn vvenc_rejects_malformed_counts_and_non_stats_noise() {
    use crate::core::encoder::Encoder;

    for line in [
        "",
        "60",
        "frame= 60",
        "noise frame= 60 fps= 900",
        "vvenc [info]: frame= 60",
        "vvenc [warning]: stats: frame= 60",
        "vvenc [info]: stats: fps= 900 elapsed= 60",
        "vvenc [info]: stats: notframe= 60 fps= 900",
        "vvenc [info]: stats: frame=",
        "vvenc [info]: stats: frame= fps= 900",
        "vvenc [info]: stats: frame= -60",
        "vvenc [info]: stats: frame= +60",
        "vvenc [info]: stats: frame= 60.0",
        "vvenc [info]: stats: frame= 60x",
        "vvenc [info]: stats: frame= 18446744073709551616",
        "vvenc [info]: stats: frame= 60/18446744073709551616",
        "vvenc [info]: stats: frame= /120",
        "vvenc [info]: stats: frame= 60/",
        "vvenc [info]: stats: frame= 60/noise",
        "vvenc [info]: stats: frame= 60/-120",
        "vvenc [info]: stats: frame= 60/+120",
        "vvenc [info]: stats: frame= 60/120/240",
        "vvenc [info]: stats: frame= 60//120",
    ] {
        assert_eq!(parse_vvc_frames(line), None, "parser: {line:?}");
        assert_eq!(
            Encoder::parse_encoded_frames_base(&EncoderBase::VVenC, line),
            None,
            "dispatcher: {line:?}"
        );
    }
}

#[test]
fn literal_help_options_and_negative_controls() {
    // Losing aliases, no- expansion, or equals trimming must break these
    // independent fixtures; native help is checked separately, not snapshotted.
    for (base, help, expected) in [
        (
            EncoderBase::X265,
            "-m/--subme <int> -w/--[no-]weightp --[no-]-hrd-concat --keyint <int> - --",
            "-m --subme -w --weightp --no-weightp --hrd-concat --no-hrd-concat --keyint",
        ),
        (
            EncoderBase::AOM,
            "--passes=<arg> --fpf=<arg> --film-grain-table=<arg> -o <arg> - --",
            "--passes --fpf --film-grain-table -o",
        ),
        (
            EncoderBase::SVTAV1,
            "-i, --input <file> --stats <file> --pass <int> --fgs-table <file>",
            "-i --input --stats --pass --fgs-table",
        ),
        (
            EncoderBase::RAV1E,
            "--first-pass <STATS_FILE> --second-pass <STATS_FILE> --quiet -y",
            "--first-pass --second-pass --quiet -y",
        ),
    ] {
        let actual = valid_params(help, base);
        assert_eq!(
            actual,
            expected.split_ascii_whitespace().map(Cow::Borrowed).collect(),
            "{base}"
        );
        assert!(!actual.contains("--cli-parity-deliberately-nonexistent"));
        assert!(!actual.contains("--passes=<arg>"));
        assert!(!actual.contains("--[no-]weightp"));
    }
}

#[test]
fn rav1e_parsing() {
    let test_cases = [
        (
            "encoded 1 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            Some(1),
        ),
        (
            "encoded 12 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            Some(12),
        ),
        (
            "encoded 122 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            Some(122),
        ),
        (
            "encoded 1220 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            Some(1220),
        ),
        (
            "encoded 12207 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            Some(12207),
        ),
        ("invalid", None),
        (
            "encoded xxxx frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s",
            None,
        ),
    ];

    for (s, ans) in test_cases {
        assert_eq!(parse_rav1e_frames(s), ans);
    }
}

#[test]
fn x26x_parsing() {
    let test_cases = [
        ("24 frames: 39.11 fps, 14.60 kb/s", Some(24)),
        ("240 frames: 39.11 fps, 14.60 kb/s", Some(240)),
        ("2445 frames: 39.11 fps, 14.60 kb/s", Some(2445)),
        ("24145 frames: 39.11 fps, 14.60 kb/s", Some(24145)),
        ("246434 frames: 39.11 fps, 14.60 kb/s", Some(246_434)),
        ("2448732 frames: 39.11 fps, 14.60 kb/s", Some(2_448_732)),
        (
            "[42.5%] 121/285 frames, 2.47 fps, 1445.28 kb/s, eta 0:01:06",
            Some(121),
        ),
        ("invalid data", None),
        ("", None),
    ];

    for (s, ans) in test_cases {
        assert_eq!(parse_x26x_frames(s), ans);
    }
}

#[test]
fn svt_av1_parsing() {
    let test_cases = [
        ("Encoding frame    0 1.08 kbps 2.00 fps", Some(0)),
        ("Encoding frame    7 1.08 kbps 2.00 fps", Some(7)),
        ("Encoding frame   22 2.03 kbps 3.68 fps", Some(22)),
        ("Encoding frame  7654 1.08 kbps 2.00 fps", Some(7654)),
        ("Encoding frame 72415 1.08 kbps 2.00 fps", Some(72415)),
        ("Encoding frame 778743 1.08 kbps 2.00 fps", Some(778_743)),
        (
            "Encoding frame 53298734 1.08 kbps 2.00 fps",
            Some(53_298_734),
        ),
        (
            "Encoding:   55 Frames @ 7.05 fps | 681.56 kb/s | Size: 0.60 MB | Time: 0:00:08",
            Some(55),
        ),
        (
            "Encoding:  145/241 Frames @ 8.54 fps | 975.70 kb/s | Size: 1.23 MB [2.04 MB] | Time: \
             0:00:17 [-0:00:11]",
            Some(145),
        ),
        (
            "Encoding: \u{1b}[33m  24/171 Frames\u{1b}[0m @ \u{1b}[32m3.43\u{1b}[0m fps | \
             \u{1b}[35m670.50 kb/s\u{1b}[0m | Size: \u{1b}[31m0.60 MB\u{1b}[0m \
             \u{1b}[38;5;248m[4.26 MB]\u{1b}[0m | Time: \u{1b}[36m0:00:07\u{1b}[0m \
             \u{1b}[38;5;248m[-0:00:43]\u{1b}[0m \r",
            Some(24),
        ),
        ("invalid input", None),
        ("", None),
    ];

    for (s, ans) in test_cases {
        assert_eq!(parse_svt_av1_frames(s), ans);
    }
}

#[test]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn aom_vpx_parsing() {
    // We return the number of frames without checking
    // if those frames are not the final pass.

    // Therefore, parse_encoded_frames shouldn't be called
    // when running the first out of two passes.
    let test_cases = [
        (
            "Pass 1/1 frame    3/2       2131B    5997 us 500.25 fps [ETA  unknown]",
            Some(2),
        ),
        (
            "Pass 2/2 frame   84/83     81091B  132314 us 634.85 fps [ETA  unknown]",
            Some(83),
        ),
        (
            "Pass 1/1 frame  142/141   156465B  208875 us 679.83 fps [ETA  unknown]",
            Some(141),
        ),
        (
            "Pass 1/2 frame  142/141   156465B  208875 us 679.83 fps [ETA  unknown]",
            Some(141),
        ),
        (
            "Pass 2/2 frame 4232/4231 5622510B 5518075 us 766.93 fps [ETA  unknown]",
            Some(4231),
        ),
        (
            "Pass 1/1 frame 13380/13379 17860525B   16760 ms 798.31 fps [ETA  unknown]",
            Some(13379),
        ),
        (
            "Pass 1/2 frame 13380/13379 17860525B   16760 ms 798.31 fps [ETA  unknown]",
            Some(13379),
        ),
        ("invalid data", None),
        (
            "Pass 2/2 frame 102262/102261 136473850B  131502 ms 777.65 fps [ETA  unknown]    1272F",
            Some(102_261),
        ),
        (
            "Pass 1/1 frame 1022621/1022611 136473850B  131502 ms 777.65 fps [ETA  unknown]    \
             1272F",
            Some(1_022_611),
        ),
        (
            "Pass 1/1 frame 10000/9999 5319227B 8663074 us 1154.32 fps [ETA  unknown]",
            Some(9999),
        ),
    ];

    if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
        for (s, ans) in test_cases {
            // SAFETY: asserted that machine has required CPU features
            assert_eq!(unsafe { parse_aom_vpx_frames_sse41(s.as_bytes()) }, ans);
        }
    }

    for (s, ans) in test_cases {
        assert_eq!(parse_aom_vpx_frames(s), ans);
    }
}
