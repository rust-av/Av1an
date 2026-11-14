//! Test-only real-header oracle. No compiler/header dependency for crate builds.
use std::{
    collections::BTreeMap,
    mem::{align_of, offset_of, size_of, size_of_val},
    path::{Path, PathBuf},
    process::Command,
};

use super::*;

#[test]
fn installed_vmaf_headers_match_rust_layouts_and_signatures() {
    check_header_layouts(false);
}

#[test]
fn inserted_picture_field_is_detected_as_offset_drift() {
    check_header_layouts(true);
}

fn check_header_layouts(drift_check: bool) {
    let Some(include) = header_directory() else {
        return;
    };
    let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
    if let Err(error) = Command::new(&compiler).arg("--version").output() {
        unavailable(&format!("C11 GCC/Clang compiler {compiler:?}: {error}"));
        return;
    }
    let directory = tempfile::tempdir().expect("temporary ABI header probe");
    eprintln!(
        "ABI: CPU headers from {}; version macros 3/2/0; CUDA public header {}",
        include.display(),
        if include.join("libvmaf/libvmaf_cuda.h").is_file() {
            "installed"
        } else {
            "pinned vendored fallback"
        }
    );
    let expected = rust_layouts();
    let actual = probe(&compiler, &include, directory.path(), None);
    assert_eq!(
        actual, expected,
        "installed C headers disagree with Rust ABI"
    );
    eprintln!(
        "ABI: {} real-header layout values match; C signatures compile",
        actual.len()
    );

    if !drift_check {
        return;
    }

    let picture = std::fs::read_to_string(include.join("libvmaf/picture.h"))
        .expect("read real picture header for controlled negative fixture");
    let altered = picture.replacen(
        "unsigned bpc;",
        "unsigned bpc;\n    unsigned abi_offset_drift;",
        1,
    );
    assert_ne!(
        picture, altered,
        "negative fixture must change the actual header"
    );
    let drift_include = directory.path().join("drift");
    std::fs::create_dir_all(drift_include.join("libvmaf")).expect("negative include directory");
    std::fs::write(drift_include.join("libvmaf/picture.h"), altered)
        .expect("write negative header");
    let drift = probe(&compiler, &include, directory.path(), Some(&drift_include));
    assert_ne!(
        drift, expected,
        "same ABI comparison MUST reject the altered real header"
    );
    assert_ne!(
        drift["VmafPicture.w.offset"], expected["VmafPicture.w.offset"],
        "detect field offset drift, not just size"
    );
    eprintln!(
        "ABI: negative header rejected: VmafPicture.w offset C={} Rust={}",
        drift["VmafPicture.w.offset"], expected["VmafPicture.w.offset"]
    );
}

fn unavailable(reason: &str) {
    assert!(
        std::env::var_os("AV_METRICS_REQUIRE_CPU").is_none()
            && std::env::var_os("VMAF_INCLUDE_DIR").is_none(),
        "ABI required header gate unavailable: {reason}. Install pinned v3.2.1 development headers and a C11 GCC/Clang compiler; set VMAF_INCLUDE_DIR to the parent of libvmaf/ and CC to the compiler."
    );
    eprintln!("UNEXECUTED ABI header gate: {reason}; set AV_METRICS_REQUIRE_CPU=1 to require it");
}

fn header_directory() -> Option<PathBuf> {
    let directory = std::env::var_os("VMAF_INCLUDE_DIR").map(PathBuf::from).or_else(|| {
        // Windows installers need an explicit include directory, not pkg-config.
        if cfg!(target_os = "windows") {
            return None;
        }
        let output = Command::new("pkg-config")
            .args(["--variable=includedir", "libvmaf"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?;
        if path.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(path.trim()))
    });
    let Some(directory) = directory else {
        unavailable("no include directory (set VMAF_INCLUDE_DIR, or install libvmaf.pc on Unix)");
        return None;
    };
    for header in ["picture.h", "libvmaf.h", "model.h", "feature.h", "version.h"] {
        if !directory.join("libvmaf").join(header).is_file() {
            unavailable(&format!("missing {header} in {}", directory.display()));
            return None;
        }
    }
    Some(directory)
}

fn probe(
    compiler: &std::ffi::OsStr,
    include: &Path,
    directory: &Path,
    drift: Option<&Path>,
) -> BTreeMap<String, usize> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let executable = directory.join(if cfg!(target_os = "windows") {
        "headers.exe"
    } else {
        "headers"
    });
    let mut command = Command::new(compiler);
    command.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
    if let Some(drift) = drift {
        command.arg("-I").arg(drift);
    }
    let output = command
        .arg("-I")
        .arg(include)
        .arg("-I")
        .arg(fixtures.join("headers"))
        .arg(fixtures.join("headers.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("execute C header compiler");
    assert!(
        output.status.success(),
        "ABI C header/signature compilation failed (include {}): {}",
        include.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(executable).output().expect("execute real-header probe");
    assert!(output.status.success(), "header probe failed");
    String::from_utf8(output.stdout)
        .expect("ASCII probe output")
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(' ').expect("header probe key/value");
            (
                key.to_owned(),
                value.parse().expect("header probe size/offset"),
            )
        })
        .collect()
}

fn rust_layouts() -> BTreeMap<String, usize> {
    let mut values = BTreeMap::new();
    macro_rules! layout {
        ($ty:ty; $($field:ident => $c_field:literal),+ $(,)?) => {{
            let name = stringify!($ty);
            values.insert(format!("{name}.size"), size_of::<$ty>());
            values.insert(format!("{name}.align"), align_of::<$ty>());
            // SAFETY: these shared types contain only integers, pointers and
            // C enums with a defined zero variant. No native pointer is followed.
            let value: $ty = unsafe { std::mem::zeroed() };
            $(values.insert(format!("{name}.{}.offset", $c_field), offset_of!($ty, $field));
              values.insert(format!("{name}.{}.size", $c_field), size_of_val(&value.$field));)+
        }};
    }
    layout!(VmafPicture; pix_fmt => "pix_fmt", bpc => "bpc", w => "w", h => "h", stride => "stride", data => "data", ref_ => "ref", priv_ => "priv");
    layout!(VmafConfiguration; log_level => "log_level", n_threads => "n_threads", n_subsample => "n_subsample", cpumask => "cpumask", gpumask => "gpumask");
    layout!(VmafModelConfig; name => "name", flags => "flags");
    layout!(VmafCudaConfiguration; cu_ctx => "cu_ctx");
    layout!(VmafPictureConfigPictureParams; w => "w", h => "h", bpc => "bpc", pix_fmt => "pix_fmt");
    layout!(VmafPictureConfiguration; pic_params => "pic_params", pic_cnt => "pic_cnt");
    layout!(VmafCudaPictureConfigPictureParams; w => "w", h => "h", bpc => "bpc", pix_fmt => "pix_fmt");
    layout!(VmafCudaPictureConfiguration; pic_params => "pic_params", pic_prealloc_method => "pic_prealloc_method");
    values
}
