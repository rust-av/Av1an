use std::mem::{offset_of, size_of, size_of_val};

use super::Dav1dFilmGrainDataRaw as Raw;

fn fixture() -> Raw {
    let mut raw = Raw {
        seed: 0x0123_4567,
        num_y_points: 2,
        y_points: [[17, 29]; 14],
        chroma_scaling_from_luma: -3,
        num_uv_points: [4, 5],
        uv_points: [[[31, 47]; 10], [[61, 79]; 10]],
        scaling_shift: -17,
        ar_coeff_lag: 3,
        ar_coeffs_y: [-128; 24],
        ar_coeffs_uv: [[-127; 28], [126; 28]],
        ar_coeff_shift: 0x0123_4567_89ab_cdef,
        grain_scale_shift: -23,
        uv_mult: [-31, 37],
        uv_luma_mult: [41, -43],
        uv_offset: [-47, 53],
        overlap_flag: 1,
        clip_to_restricted_range: -59,
    };
    raw.y_points[0] = [1, 2];
    raw.y_points[13] = [253, 254];
    raw.uv_points[0][9] = [251, 252];
    raw.uv_points[1][9] = [253, 254];
    raw.ar_coeffs_y[23] = 127;
    raw.ar_coeffs_uv[0][27] = 127;
    raw.ar_coeffs_uv[1][27] = -128;
    raw
}

fn fields(raw: &Raw) -> Vec<(&'static str, usize, usize)> {
    macro_rules! fields {
        ($($field:ident),+ $(,)?) => {
            vec![$((stringify!($field), offset_of!(Raw, $field), size_of_val(&raw.$field))),+]
        };
    }
    fields!(
        seed,
        num_y_points,
        y_points,
        chroma_scaling_from_luma,
        num_uv_points,
        uv_points,
        scaling_shift,
        ar_coeff_lag,
        ar_coeffs_y,
        ar_coeffs_uv,
        ar_coeff_shift,
        grain_scale_shift,
        uv_mult,
        uv_luma_mult,
        uv_offset,
        overlap_flag,
        clip_to_restricted_range,
    )
}

#[test]
fn fields_padding_and_moves_are_deterministic() {
    let raw = fixture();
    let bytes = raw.to_bytes();
    assert_eq!(bytes.len(), size_of::<Raw>());
    macro_rules! field {
        ($field:ident, $expected:expr) => {{
            let expected = $expected;
            let start = offset_of!(Raw, $field);
            assert_eq!(
                &bytes[start..start + expected.len()],
                expected,
                stringify!($field)
            );
        }};
    }
    field!(seed, 0x0123_4567u32.to_ne_bytes());
    field!(num_y_points, 2i32.to_ne_bytes());
    let mut y = [17, 29].repeat(14);
    y[..2].copy_from_slice(&[1, 2]);
    y[26..].copy_from_slice(&[253, 254]);
    field!(y_points, y);
    field!(chroma_scaling_from_luma, (-3i32).to_ne_bytes());
    field!(
        num_uv_points,
        [4i32.to_ne_bytes(), 5i32.to_ne_bytes()].concat()
    );
    let mut uv = [[31, 47].repeat(10), [61, 79].repeat(10)].concat();
    uv[18..20].copy_from_slice(&[251, 252]);
    uv[38..40].copy_from_slice(&[253, 254]);
    field!(uv_points, uv);
    field!(scaling_shift, (-17i32).to_ne_bytes());
    field!(ar_coeff_lag, 3i32.to_ne_bytes());
    let mut y_coeff = [128; 24];
    y_coeff[23] = 127;
    field!(ar_coeffs_y, y_coeff);
    let mut uv_coeff = [[129; 28], [126; 28]].concat();
    uv_coeff[27] = 127;
    uv_coeff[55] = 128;
    field!(ar_coeffs_uv, uv_coeff);
    field!(ar_coeff_shift, 0x0123_4567_89ab_cdefu64.to_ne_bytes());
    field!(grain_scale_shift, (-23i32).to_ne_bytes());
    field!(
        uv_mult,
        [(-31i32).to_ne_bytes(), 37i32.to_ne_bytes()].concat()
    );
    field!(
        uv_luma_mult,
        [41i32.to_ne_bytes(), (-43i32).to_ne_bytes()].concat()
    );
    field!(
        uv_offset,
        [(-47i32).to_ne_bytes(), 53i32.to_ne_bytes()].concat()
    );
    field!(overlap_flag, 1i32.to_ne_bytes());
    field!(clip_to_restricted_range, (-59i32).to_ne_bytes());

    let mut occupied = vec![false; bytes.len()];
    for (_, offset, len) in fields(&raw) {
        occupied[offset..offset + len].fill(true);
    }
    for (offset, is_field) in occupied.into_iter().enumerate() {
        if !is_field {
            assert_eq!(bytes[offset], 0, "padding byte {offset}");
        }
    }
    let mut moved = raw;
    for pattern in [0, 0x55, 0xaa, 0xff] {
        let stack = std::hint::black_box([pattern; 4096]);
        moved = std::hint::black_box(moved.clone());
        assert_eq!(moved.to_bytes(), bytes);
        std::hint::black_box(stack);
    }
}

// Miri checks the same production writer above without processes or native dependencies.
#[cfg(not(miri))]
#[test]
fn matches_installed_dav1d_c_header() {
    use std::{fmt::Write, fs, mem::align_of, process::Command, time::SystemTime};

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove dav1d C-probe scratch directory");
        }
    }
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock is after Unix epoch")
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("grain-layout-dav1d-{}-{nonce}", std::process::id()));
    #[allow(
        clippy::create_dir,
        reason = "Never reuse or clean up an existing directory"
    )]
    fs::create_dir(&directory).expect("create exclusive C-probe scratch directory");
    let scratch = Scratch(directory);
    let source = scratch.0.join("probe.c");
    let executable = scratch.0.join("probe");
    fs::write(&source, include_str!("../layout_probe.c")).expect("write C probe");
    let compiler = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("grain-layout ABI proof requires cc and dav1d API 7 development headers");
    assert!(
        compiler.status.success(),
        "dav1d ABI probe failed: {}",
        String::from_utf8_lossy(&compiler.stderr)
    );
    let output = Command::new(&executable).output().expect("run dav1d C probe");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("C probe emits ASCII");
    println!("dav1d C oracle:\n{text}");
    let mut lines = text.lines();
    assert_eq!(
        lines.next().expect("C layout summary"),
        format!(
            "layout {} {} {} {}",
            size_of::<Raw>(),
            align_of::<Raw>(),
            size_of::<i32>(),
            size_of::<u32>()
        )
    );
    assert!(
        lines.next().expect("C API version").starts_with("api 7 "),
        "unsupported dav1d API major"
    );
    let raw = fixture();
    for (name, offset, len) in fields(&raw) {
        assert_eq!(
            lines.next().expect("C field layout"),
            format!("{name} {offset} {len}"),
            "C layout mismatch for {name}"
        );
    }
    let expected_hex = lines.next().expect("C fixture bytes");
    let mut actual_hex = String::new();
    for byte in raw.to_bytes() {
        write!(&mut actual_hex, "{byte:02x}").expect("write hex to string");
    }
    assert_eq!(
        actual_hex, expected_hex,
        "serialized fields differ from initialized C oracle"
    );
    assert!(lines.next().is_none());
}
