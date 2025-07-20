use crate::parse::*;

#[test]
fn valid_params_works() {
    use std::borrow::Borrow;

    macro_rules! generate_tests {
      ($($x:ident),* $(,)?) => {
        $(
          let returned: HashSet<String> = valid_params(include_str!(concat!("../../tests/", stringify!($x), "_help.txt")), Encoder::$x)
            .iter()
            .map(|s| s.to_string())
            .collect();
          let expected: HashSet<String> = include_str!(concat!("../../tests/", stringify!($x), "_params.txt"))
            .split_ascii_whitespace()
            .map(|s| s.to_string())
            .collect();

          for arg in expected.iter() {
            assert!(returned.contains(Borrow::<str>::borrow(&**arg)), "expected '{}', but it was missing in return value (for encoder {})", arg, stringify!($x));
          }
          for arg in returned.iter() {
            assert!(expected.contains(Borrow::<str>::borrow(&**arg)), "return value contains '{}', but it was not expected (for encoder {})", arg, stringify!($x));
          }
          assert_eq!(returned, expected);
        )*
      };
    }

    generate_tests!(aom, rav1e, svt_av1, vpx, x264, x265);
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
