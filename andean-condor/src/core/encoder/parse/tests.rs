use crate::core::encoder::parse::*;

#[test]
fn aom_frame_count_rejects_signed_nondecimal_and_overflow_values() {
    for count in ["+1", "-1", "", "1x", "1.0", "１", "18446744073709551616"] {
        let line = format!("Pass 1/1 frame    1/{count} 12345678901234567890B");
        assert_eq!(parse_aom_vpx_frames(&line), None, "count: {count:?}");
    }
}

#[test]
fn vvenc_stats_frame_key_is_parsed_with_optional_totals_and_percentages() {
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
            EncoderBase::VVenC.parse_encoded_frames(line),
            Some(expected),
            "dispatcher: {line:?}"
        );
    }
}

#[test]
fn vvenc_rejects_malformed_counts_and_non_stats_noise() {
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
            EncoderBase::VVenC.parse_encoded_frames(line),
            None,
            "dispatcher: {line:?}"
        );
    }
}

#[test]
fn help_parser_expands_native_option_spellings_without_inventing_flags() {
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
fn rav1e_progress_extracts_encoded_frames_and_ignores_noise() {
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
fn x264_and_x265_progress_extracts_current_frame_counts() {
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
fn svt_av1_progress_extracts_encoded_frame_counts() {
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
fn aom_and_vpx_progress_scalar_and_simd_counts_agree() {
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
