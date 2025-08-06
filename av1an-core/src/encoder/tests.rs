use crate::encoder::parse_svt_av1_version;

#[test]
fn svt_av1_parsing() {
    let test_cases = [
        ("SVT-AV1 v0.8.7-333-g010c1881 (release)", Some((0, 8, 7))),
        ("SVT-AV1 v0.9.0-dirty (debug)", Some((0, 9, 0))),
        ("SVT-AV1 v1.2.0 (release)", Some((1, 2, 0))),
        ("SVT-AV1 v3.2.1 (release)", Some((3, 2, 1))),
        ("SVT-AV1 v3.2.11 (release)", Some((3, 2, 11))),
        ("SVT-AV1 v0.8.11 (release)", Some((0, 8, 11))),
        ("SVT-AV1 v0.8.11-333-g010c1881 (release)", Some((0, 8, 11))),
        ("invalid", None),
    ];

    for (s, ans) in test_cases {
        assert_eq!(parse_svt_av1_version(s.as_bytes()), ans);
    }
}
