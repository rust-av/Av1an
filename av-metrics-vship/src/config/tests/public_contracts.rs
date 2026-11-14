//! Public pure contracts retained from the former correctness target.
use super::*;
use std::mem::{align_of, size_of};

#[test]
fn public_colorspace_size_and_alignment_match_rust_c_layout_mirror() {
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
fn sample_discriminants_are_non_contiguous_and_explicit() {
    assert_eq!(VshipSample::Float as i32, 0);
    assert_eq!(VshipSample::Half as i32, 1);
    assert_eq!(VshipSample::Uint8 as i32, 2);
    assert_eq!(VshipSample::Uint9 as i32, 3);
    assert_eq!(VshipSample::Uint10 as i32, 5);
    assert_eq!(VshipSample::Uint12 as i32, 7);
    assert_eq!(VshipSample::Uint14 as i32, 9);
    assert_eq!(VshipSample::Uint16 as i32, 11);
}

#[test]
fn public_colorspace_leading_fields_have_expected_literal_offsets() {
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
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.sample).cast::<u8>()),
        32
    );
}

#[test]
fn bit_depths_map_to_the_vship_sample_enum() {
    assert_eq!(sample_for_bit_depth(8), Some(VshipSample::Uint8));
    assert_eq!(sample_for_bit_depth(9), Some(VshipSample::Uint9));
    assert_eq!(sample_for_bit_depth(10), Some(VshipSample::Uint10));
    assert_eq!(sample_for_bit_depth(12), Some(VshipSample::Uint12));
    assert_eq!(sample_for_bit_depth(14), Some(VshipSample::Uint14));
    assert_eq!(sample_for_bit_depth(16), Some(VshipSample::Uint16));
    assert_eq!(sample_for_bit_depth(11), None);
    assert_eq!(sample_for_bit_depth(13), None);
}

#[test]
fn chroma_sampling_maps_to_the_vship_subsampling() {
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv420),
        Some(VshipChromaSubsample::yuv420())
    );
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv422),
        Some(VshipChromaSubsample::yuv422())
    );
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv444),
        Some(VshipChromaSubsample::yuv444())
    );
    assert_eq!(subsample_for_chroma(ChromaSubsampling::Monochrome), None);
}

#[test]
fn colorspace_mapping_uses_limited_range_and_bt709() {
    let colorspace =
        colorspace_from_details(&details(1920, 1080, 10, ChromaSubsampling::Yuv420), None)
            .expect("10-bit 4:2:0 is supported");
    assert_eq!(colorspace.width, 1920);
    assert_eq!(colorspace.height, 1080);
    assert_eq!(colorspace.sample, VshipSample::Uint10);
    assert_eq!(colorspace.range, VshipRange::Limited);
    assert_eq!(colorspace.color_family, VshipColorFamily::YUV);
    assert_eq!(colorspace.yuv_matrix, VshipYuvMatrix::Bt709);
    assert_eq!(colorspace.transfer_function, VshipTransferFunction::Bt709);
    assert_eq!(colorspace.primaries, VshipPrimaries::Bt709);
}

#[test]
fn target_resolution_maps_to_the_colorspace_target() {
    let colorspace = colorspace_from_details(
        &details(1920, 1080, 8, ChromaSubsampling::Yuv420),
        Some((1280, 720)),
    )
    .expect("8-bit 4:2:0 is supported");
    assert_eq!(colorspace.width, 1920, "the input geometry is unchanged");
    assert_eq!(colorspace.target_width, 1280);
    assert_eq!(colorspace.target_height, 720);
}

#[test]
fn an_absent_target_resolution_means_no_scaling() {
    let colorspace =
        colorspace_from_details(&details(640, 480, 8, ChromaSubsampling::Yuv420), None)
            .expect("8-bit 4:2:0 is supported");
    assert_eq!(colorspace.target_width, -1);
    assert_eq!(colorspace.target_height, -1);
}

#[test]
fn public_colorspace_mapping_rejects_eleven_bit_and_monochrome() {
    assert!(matches!(
        colorspace_from_details(&details(1920, 1080, 11, ChromaSubsampling::Yuv420), None),
        Err(VshipError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        colorspace_from_details(&details(1920, 1080, 8, ChromaSubsampling::Monochrome), None),
        Err(VshipError::UnsupportedFormat { .. })
    ));
}

#[test]
fn differing_inputs_produce_independent_colorspaces() {
    let reference = colorspace_from_details(
        &details(1920, 1080, 8, ChromaSubsampling::Yuv420),
        Some((1280, 720)),
    )
    .unwrap();
    let encode = colorspace_from_details(
        &details(1280, 720, 10, ChromaSubsampling::Yuv422),
        Some((1280, 720)),
    )
    .unwrap();
    assert_eq!(reference.sample, VshipSample::Uint8);
    assert_eq!(encode.sample, VshipSample::Uint10);
    assert_eq!(reference.width, 1920);
    assert_eq!(encode.width, 1280);
    assert_eq!(reference.target_width, encode.target_width);
}
