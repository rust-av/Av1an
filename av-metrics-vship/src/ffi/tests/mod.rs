mod internal_integration;

use std::mem::{align_of, size_of};

use super::*;

#[test]
fn version_backend_and_score_tag_fields_preserve_unknown_integers() {
    let version = VshipVersion {
        major: 5,
        minor: 1,
        minor_minor: 2,
        backend: 777,
    };
    let score = VshipScoreSsimulacra2 {
        struct_type: -123,
        score: 42.0,
    };
    assert_eq!(version.backend, 777);
    assert_eq!(score.struct_type, -123);
}

#[test]
fn known_backend_integers_roundtrip_to_names() {
    for (raw, name) in ["HIP", "CUDA", "Vulkan", "CPU"].into_iter().enumerate() {
        let backend = VshipBackend::try_from(raw as c_int).expect("known backend");
        assert_eq!(backend as c_int, raw as c_int);
        assert_eq!(backend.as_str(), name);
    }
}

#[test]
fn unknown_backend_errors_retain_raw_integer() {
    for raw in [777, -123] {
        assert!(
            VshipBackend::try_from(raw)
                .expect_err("unknown backend")
                .to_string()
                .contains(&raw.to_string())
        );
    }
}

/// Guards the FFI layouts against accidental reordering. A wrong field
/// order still compiles, and surfaces only as an opaque status from
/// libvship at runtime, which is exactly the bug these tests exist to
/// prevent.
#[test]
fn colorspace_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_Colorspace_t` in VshipColor.h.
    #[repr(C)]
    struct Expected {
        width: i64,
        height: i64,
        target_width: i64,
        target_height: i64,
        sample: VshipSample,
        range: VshipRange,
        subsampling: VshipChromaSubsample,
        chroma_location: VshipChromaLocation,
        color_family: VshipColorFamily,
        yuv_matrix: VshipYuvMatrix,
        transfer_function: VshipTransferFunction,
        primaries: VshipPrimaries,
        crop: VshipCropRectangle,
    }

    assert_eq!(size_of::<VshipColorspace>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipColorspace>(), align_of::<Expected>());
}

#[test]
fn colorspace_leading_fields_have_expected_literal_offsets() {
    let colorspace = VshipColorspace::bt709_default(1920, 1080);
    let base = std::ptr::from_ref(&colorspace).cast::<u8>() as usize;
    let offset = |field: *const u8| field as usize - base;

    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.width).cast::<u8>()),
        0
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.height).cast::<u8>()),
        8
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.target_width).cast::<u8>()),
        16
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.target_height).cast::<u8>()),
        24
    );
    // Four int64 fields precede the first enum, so the enums begin at 32.
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.sample).cast::<u8>()),
        32
    );
}

#[test]
fn sample_discriminants_match_documented_literal_values() {
    assert_eq!(VshipSample::Float as c_int, 0);
    assert_eq!(VshipSample::Half as c_int, 1);
    assert_eq!(VshipSample::Uint8 as c_int, 2);
    assert_eq!(VshipSample::Uint9 as c_int, 3);
    assert_eq!(VshipSample::Uint10 as c_int, 5);
    assert_eq!(VshipSample::Uint12 as c_int, 7);
    assert_eq!(VshipSample::Uint14 as c_int, 9);
    assert_eq!(VshipSample::Uint16 as c_int, 11);
}

#[test]
fn backend_and_color_enums_match_documented_literal_discriminants() {
    assert_eq!(VshipBackend::Hip as c_int, 0);
    assert_eq!(VshipBackend::Cuda as c_int, 1);
    assert_eq!(VshipBackend::Vulkan as c_int, 2);
    assert_eq!(VshipBackend::Cpu as c_int, 3);

    assert_eq!(VshipRange::Limited as c_int, 0);
    assert_eq!(VshipRange::Full as c_int, 1);

    assert_eq!(VshipChromaLocation::Left as c_int, 0);
    assert_eq!(VshipChromaLocation::Center as c_int, 1);
    assert_eq!(VshipChromaLocation::TopLeft as c_int, 2);
    assert_eq!(VshipChromaLocation::Top as c_int, 3);

    assert_eq!(VshipColorFamily::YUV as c_int, 0);
    assert_eq!(VshipColorFamily::RGB as c_int, 1);

    assert_eq!(VshipYuvMatrix::Rgb as c_int, 0);
    assert_eq!(VshipYuvMatrix::Bt709 as c_int, 1);
    assert_eq!(VshipYuvMatrix::Bt470Bg as c_int, 5);
    assert_eq!(VshipYuvMatrix::St170M as c_int, 6);
    assert_eq!(VshipYuvMatrix::Ycgco as c_int, 8);
    assert_eq!(VshipYuvMatrix::Bt2020Ncl as c_int, 9);
    assert_eq!(VshipYuvMatrix::Bt2020Cl as c_int, 10);
    assert_eq!(VshipYuvMatrix::Bt2100Ictcp as c_int, 14);
    assert_eq!(VshipYuvMatrix::YcgcoRe as c_int, 16);
    assert_eq!(VshipYuvMatrix::YcgcoRo as c_int, 17);

    assert_eq!(VshipTransferFunction::Bt709 as c_int, 1);
    assert_eq!(VshipTransferFunction::Linear as c_int, 8);
    assert_eq!(VshipTransferFunction::Pq as c_int, 16);
    assert_eq!(VshipTransferFunction::Hlg as c_int, 18);

    assert_eq!(VshipPrimaries::Internal as c_int, -1);
    assert_eq!(VshipPrimaries::Bt709 as c_int, 1);
    assert_eq!(VshipPrimaries::Bt2020 as c_int, 9);
    assert_eq!(VshipPrimaries::DisplayP3 as c_int, 12);
}

#[test]
fn exception_discriminants_and_success_flag_match_documented_values() {
    assert_eq!(VshipException::NoError as c_int, 0);
    assert_eq!(VshipException::OutOfVram as c_int, 1);
    assert_eq!(VshipException::OutOfRam as c_int, 2);
    assert_eq!(VshipException::BadDisplayModel as c_int, 3);
    assert_eq!(VshipException::DifferingInputType as c_int, 4);
    assert_eq!(VshipException::NonRgbsInput as c_int, 5);
    assert_eq!(VshipException::DeviceCountError as c_int, 6);
    assert_eq!(VshipException::NoDeviceDetected as c_int, 7);
    assert_eq!(VshipException::BadDeviceArgument as c_int, 8);
    assert_eq!(VshipException::BadDeviceCode as c_int, 9);
    assert_eq!(VshipException::BadHandler as c_int, 10);
    assert_eq!(VshipException::BadPointer as c_int, 11);
    assert_eq!(VshipException::HipError as c_int, 12);
    assert_eq!(VshipException::BadPath as c_int, 13);
    assert_eq!(VshipException::BadJson as c_int, 14);
    assert_eq!(VshipException::NotSupported as c_int, 15);
    assert_eq!(VshipException::BadInputStructType as c_int, 16);
    assert_eq!(VshipException::BadErrorType as c_int, 17);

    assert!(VshipException::NoError.is_ok());
    assert!(!VshipException::BadHandler.is_ok());
}

#[test]
fn struct_type_discriminants_match_documented_literal_values() {
    assert_eq!(VshipStructType::Null as c_int, 0);
    assert_eq!(VshipStructType::InitSsimulacra2 as c_int, 1);
    assert_eq!(VshipStructType::InitButteraugli as c_int, 2);
    assert_eq!(VshipStructType::InitCvvdp as c_int, 3);
    assert_eq!(VshipStructType::ScoreSsimulacra2 as c_int, 4);
    assert_eq!(VshipStructType::ScoreButteraugli as c_int, 5);
    assert_eq!(VshipStructType::ScoreCvvdp as c_int, 6);
}

#[test]
fn ssimulacra2_init_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_InitSSIMULACRA2_1` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        src_colorspace: VshipColorspace,
        dis_colorspace: VshipColorspace,
        gpu_id: c_int,
    }

    assert_eq!(size_of::<VshipInitSsimulacra2_1>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipInitSsimulacra2_1>(), align_of::<Expected>());
}

#[test]
fn butteraugli_init_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_InitButteraugli_1` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        src_colorspace: VshipColorspace,
        dis_colorspace: VshipColorspace,
        q_norm: c_int,
        intensity_multiplier: f32,
        gpu_id: c_int,
    }

    assert_eq!(size_of::<VshipInitButteraugli_1>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipInitButteraugli_1>(), align_of::<Expected>());
}

#[test]
fn cvvdp_init_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_InitCVVDP_1` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        src_colorspace: VshipColorspace,
        dis_colorspace: VshipColorspace,
        fps: f32,
        resize_to_display: bool,
        model_key_cstr: *const c_char,
        model_config_json_cstr: *const c_char,
        gpu_id: c_int,
    }

    assert_eq!(size_of::<VshipInitCvvdp_1>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipInitCvvdp_1>(), align_of::<Expected>());
}

#[test]
fn ssimulacra2_score_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_ScoreSSIMULACRA2` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        score: f64,
    }

    assert_eq!(size_of::<VshipScoreSsimulacra2>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipScoreSsimulacra2>(), align_of::<Expected>());
}

#[test]
fn butteraugli_score_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_ScoreButteraugli` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        norm_q: f64,
        norm_3: f64,
        norm_inf: f64,
        dstp: *const u8,
        dst_stride: i64,
    }

    assert_eq!(size_of::<VshipScoreButteraugli>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipScoreButteraugli>(), align_of::<Expected>());
}

#[test]
fn cvvdp_score_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_ScoreCVVDP` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        struct_type: VshipStructType,
        score: f64,
        dstp: *const u8,
        dst_stride: i64,
    }

    assert_eq!(size_of::<VshipScoreCvvdp>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipScoreCvvdp>(), align_of::<Expected>());
}

#[test]
fn version_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors the anonymous `Vship_Version` struct in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        major: c_int,
        minor: c_int,
        minor_minor: c_int,
        backend: VshipBackend,
    }

    assert_eq!(size_of::<VshipVersion>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipVersion>(), align_of::<Expected>());
}

#[test]
fn device_info_size_and_alignment_match_rust_c_layout_mirror() {
    // Mirrors `typedef struct Vship_DeviceInfo` in VshipAPI.h.
    #[repr(C)]
    struct Expected {
        name: [c_char; 256],
        vram_size: u64,
        integrated: c_int,
        multi_processor_count: c_int,
        warp_size: c_int,
        vulkan_feature_matrix: VshipVulkanFeatureMatrix,
    }

    assert_eq!(size_of::<VshipDeviceInfo>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipDeviceInfo>(), align_of::<Expected>());
}

#[test]
fn device_info_name_stops_at_the_nul() {
    let mut info = VshipDeviceInfo::default();
    // The C header declares `name` as `char`, which is signed, so each
    // ASCII byte must be reinterpreted rather than assigned
    // directly.
    for (slot, byte) in info.name[..5].iter_mut().zip(*b"GeFoo") {
        *slot = byte as c_char;
    }
    assert_eq!(info.name(), "GeFoo");
    assert_eq!(VshipDeviceInfo::default().name(), "");
}
