#[cfg(unix)]
mod internal_integration;

use super::*;

#[test]
fn unsupported_formats_are_rejected() {
    for (bit_depth, chroma_sampling) in [
        (9u32, ChromaSubsampling::Yuv420),
        (14, ChromaSubsampling::Yuv420),
        (8, ChromaSubsampling::Monochrome),
    ] {
        let format = VideoFormat {
            width: 64,
            height: 48,
            bit_depth,
            chroma_sampling,
        };
        assert!(
            format.validate().is_err(),
            "bit depth {bit_depth} with {chroma_sampling:?} must be rejected"
        );
    }
}

// Only host buffers are used: no native library, model, GPU or native ownership.
fn host_picture(buffers: &mut [Vec<u8>; 3], width: u32, height: u32) -> VmafPicture {
    let mut picture = VmafPicture::zeroed();
    picture.pix_fmt = VmafPixelFormat::Yuv444p as std::ffi::c_int;
    picture.w = [width; 3];
    picture.h = [height; 3];
    picture.stride = [isize::try_from(buffers[0].len() / height as usize).unwrap(); 3];
    picture.data = std::array::from_fn(|index| buffers[index].as_mut_ptr().cast());
    picture
}

#[test]
fn unknown_distorted_pool_format_rejects_before_either_copy() {
    let bytes = [7u8; 4];
    let source = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
    let mut reference = std::array::from_fn(|_| vec![0xa5; 4]);
    let mut distorted = reference.clone();
    let mut a = host_picture(&mut reference, 2, 2);
    let mut b = host_picture(&mut distorted, 2, 2);
    b.pix_fmt = 777;
    assert!(
        VmafScorer::fill_pictures(
            &mut a,
            &mut b,
            &[source; 3],
            &[source; 3],
            Some(VmafPixelFormat::Yuv444p),
            8
        )
        .is_err()
    );
    assert!(
        reference.iter().chain(&distorted).flatten().all(|&b| b == 0xa5),
        "preflight both native tags before either copy"
    );
}

#[test]
fn fill_rejects_u8_for_ten_bit_before_copy() {
    let bytes = [7u8; 4];
    let source = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    assert!(matches!(
        VmafScorer::fill_picture(
            &mut picture,
            &[source; 3],
            Some(VmafPixelFormat::Yuv444p),
            10
        ),
        Err(VmafError::InputMismatch { .. })
    ));
    assert!(buffers.iter().flatten().all(|&byte| byte == 0xa5));
}

#[test]
fn bottom_up_rows_use_signed_byte_offsets() {
    let bytes = [1u8, 2, 0, 0, 3, 4, 0, 0];
    let source = PlaneSource::from_bytes(&bytes, 2, 2, -4, 4, 1).unwrap();
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 8]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    VmafScorer::fill_picture(
        &mut picture,
        &[source; 3],
        Some(VmafPixelFormat::Yuv444p),
        8,
    )
    .unwrap();
    for buffer in buffers {
        assert_eq!(buffer, [3, 4, 0xa5, 0xa5, 1, 2, 0xa5, 0xa5]);
    }
}

#[test]
fn fill_preserves_visible_rows_and_padding_across_widths_depths_and_row_directions() {
    for width in [1usize, 2, 7, 15, 16, 17, 64] {
        for (depth, sample) in [(8, 1), (10, 2), (12, 2), (16, 2)] {
            let first = if sample == 1 {
                vec![17; width]
            } else {
                [0x123u16].repeat(width).into_iter().flat_map(u16::to_ne_bytes).collect()
            };
            let second = if sample == 1 {
                vec![51; width]
            } else {
                [0x256u16].repeat(width).into_iter().flat_map(u16::to_ne_bytes).collect()
            };
            let row_bytes = width * sample;
            let stride = row_bytes + 3;
            // Odd byte origin/stride deliberately exercise unaligned wide copies.
            let mut bytes = vec![0xee; 1 + stride + row_bytes];
            bytes[1..1 + row_bytes].copy_from_slice(&first);
            bytes[1 + stride..].copy_from_slice(&second);
            for bottom_up in [false, true] {
                let (spacing, origin) = if bottom_up {
                    (-(stride as isize), 1 + stride)
                } else {
                    (stride as isize, 1)
                };
                let source =
                    PlaneSource::from_bytes(&bytes, width, 2, spacing, origin, sample).unwrap();
                let mut buffers = std::array::from_fn(|_| vec![0xa5; 2 * stride]);
                let mut picture = host_picture(&mut buffers, width as u32, 2);
                VmafScorer::fill_picture(
                    &mut picture,
                    &[source; 3],
                    Some(VmafPixelFormat::Yuv444p),
                    depth,
                )
                .unwrap();
                let (row0, row1) = if bottom_up {
                    (&second, &first)
                } else {
                    (&first, &second)
                };
                for buffer in buffers {
                    assert_eq!(&buffer[..row_bytes], row0);
                    assert_eq!(&buffer[stride..stride + row_bytes], row1);
                    assert_eq!(&buffer[row_bytes..stride], &[0xa5; 3]);
                    assert_eq!(&buffer[stride + row_bytes..], &[0xa5; 3]);
                }
                assert_eq!(picture.bpc, depth);
            }
        }
    }
}

#[test]
fn plane_layout_rejects_invalid_spans_strides_origins_dimensions_and_sample_sizes() {
    let bytes = [0u8; 16];
    for (len, width, height, stride, origin, sample) in [
        (5, 2, 2, -4, 4, 1),  // visible extent is 6, not the full padded 8
        (11, 2, 2, -8, 8, 2), // visible extent is 12, not the full padded 16
        (5, 2, 2, 4, 0, 1),
        (11, 2, 2, 8, 0, 2),
        (16, 2, 2, 1, 0, 1),
        (16, 2, 2, -3, 4, 2),
        (16, 2, 2, -4, 3, 1),
        (16, 2, 2, isize::MIN, 4, 1),
        (16, 1, 1, isize::MIN, 0, 1),
        (16, 1, 3, isize::MAX, 0, 1),
        (16, 1, 2, 1, isize::MAX as usize, 1),
        (16, 1, 1, 1, usize::MAX, 1),
        (16, usize::MAX, 1, 2, 0, 2),
        (16, 1, usize::MAX, 1, 0, 1),
        (16, 0, 2, 2, 0, 1),
        (16, 2, 0, 2, 0, 1),
        (16, 2, 2, 4, 0, 0),
        (16, 2, 2, 6, 0, 3),
    ] {
        assert!(
            matches!(
                PlaneSource::from_bytes(&bytes[..len], width, height, stride, origin, sample),
                Err(VmafError::MalformedFrame { .. })
            ),
            "layout {len}/{width}/{height}/{stride}/{origin}/{sample}"
        );
    }
}

#[test]
fn bottom_up_layout_accepts_exact_visible_extent() {
    let bytes = [0u8; 16];
    assert!(PlaneSource::from_bytes(&bytes[..6], 2, 2, -4, 4, 1).is_ok());
    assert!(PlaneSource::from_bytes(&bytes[..12], 2, 2, -8, 8, 2).is_ok());
}

#[test]
fn raw_plane_rejects_null_and_unrepresentable_spans() {
    let bytes = [0u8; 16];
    // SAFETY: these invalid declarations must be rejected without any dereference.
    unsafe {
        assert!(PlaneSource::from_raw_parts(std::ptr::null(), 4, 2, 2, 2, 0, 1).is_err());
        assert!(PlaneSource::from_raw_parts(bytes.as_ptr(), usize::MAX, 2, 2, 2, 0, 1).is_err());
    }
}

#[test]
fn fill_validates_every_plane_before_writing_any_plane() {
    let bytes = [17u8; 16];
    let good = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
    let wide = PlaneSource::from_bytes(&bytes, 2, 2, 4, 0, 2).unwrap();
    let wrong_width = PlaneSource::from_bytes(&bytes, 1, 2, 2, 0, 1).unwrap();
    let wrong_height = PlaneSource::from_bytes(&bytes, 2, 1, 2, 0, 1).unwrap();
    for plane_index in 0..3 {
        for invalid in [wide, wrong_width, wrong_height] {
            let mut planes = [good; 3];
            planes[plane_index] = invalid;
            let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
            let mut picture = host_picture(&mut buffers, 2, 2);
            assert!(matches!(
                VmafScorer::fill_picture(&mut picture, &planes, Some(VmafPixelFormat::Yuv444p), 8),
                Err(VmafError::InputMismatch { .. })
            ));
            assert!(buffers.iter().flatten().all(|&byte| byte == 0xa5));
        }
    }
    for depth in [10, 12, 16] {
        for plane_index in 0..3 {
            let mut planes = [wide; 3];
            planes[plane_index] = good;
            let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
            let mut picture = host_picture(&mut buffers, 2, 2);
            assert!(matches!(
                VmafScorer::fill_picture(
                    &mut picture,
                    &planes,
                    Some(VmafPixelFormat::Yuv444p),
                    depth
                ),
                Err(VmafError::InputMismatch { .. })
            ));
            assert!(buffers.iter().flatten().all(|&byte| byte == 0xa5));
        }
    }
}

#[test]
fn pair_rejects_either_side_before_copying_either_picture() {
    let bytes = [17u8; 16];
    let narrow = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
    let wide = PlaneSource::from_bytes(&bytes, 2, 2, 4, 0, 2).unwrap();
    for depth in [8, 10, 12, 16] {
        let (good, wrong) = if depth == 8 {
            (narrow, wide)
        } else {
            (wide, narrow)
        };
        for side in 0..2 {
            for index in 0..3 {
                let mut inputs = [[good; 3]; 2];
                inputs[side][index] = wrong;
                let mut reference = std::array::from_fn(|_| vec![0xa5; 16]);
                let mut distorted = std::array::from_fn(|_| vec![0xa5; 16]);
                let mut reference_picture = host_picture(&mut reference, 2, 2);
                let mut distorted_picture = host_picture(&mut distorted, 2, 2);
                assert!(matches!(
                    VmafScorer::fill_pictures(
                        &mut reference_picture,
                        &mut distorted_picture,
                        &inputs[0],
                        &inputs[1],
                        Some(VmafPixelFormat::Yuv444p),
                        depth,
                    ),
                    Err(VmafError::InputMismatch { .. })
                ));
                assert!(reference.iter().chain(&distorted).flatten().all(|&byte| byte == 0xa5));
            }
        }
    }
}

#[test]
fn fill_rejects_malformed_host_destination_without_writes() {
    let bytes = [17u8; 4];
    let source = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
    for stride in [-1, 1, isize::MAX] {
        let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
        let mut picture = host_picture(&mut buffers, 2, 2);
        picture.stride[2] = stride;
        assert!(matches!(
            VmafScorer::fill_picture(
                &mut picture,
                &[source; 3],
                Some(VmafPixelFormat::Yuv444p),
                8
            ),
            Err(VmafError::MalformedFrame { .. })
        ));
        assert!(buffers.iter().flatten().all(|&byte| byte == 0xa5));
    }
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    picture.data[2] = std::ptr::null_mut();
    assert!(
        VmafScorer::fill_picture(
            &mut picture,
            &[source; 3],
            Some(VmafPixelFormat::Yuv444p),
            8
        )
        .is_err()
    );
    assert!(buffers.iter().flatten().all(|&byte| byte == 0xa5));
}

#[test]
fn padded_u16_frame_scales_nonzero_origin_and_copies_visible_rows() {
    let mut frame = v_frame::frame::FrameBuilder::new(2, 2, ChromaSubsampling::Yuv444, 10)
        .luma_padding_left(2)
        .luma_padding_right(2)
        .luma_padding_top(2)
        .luma_padding_bottom(2)
        .build::<u16>()
        .unwrap();
    frame.y_plane.row_mut(0).unwrap().copy_from_slice(&[0x123, 0x234]);
    frame.y_plane.row_mut(1).unwrap().copy_from_slice(&[0x345, 0x156]);
    let source = plane_source(&frame.y_plane).unwrap();
    let geometry = frame.y_plane.geometry();
    assert!(geometry.data_origin() > 0);
    assert!(geometry.stride() > 2);
    assert_eq!(source.data_origin(), geometry.data_origin() * 2);
    assert_eq!(source.stride() as usize, geometry.stride() * 2);
    assert_eq!(
        source.readable_len(),
        std::mem::size_of_val(frame.y_plane.data())
    );
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    VmafScorer::fill_picture(
        &mut picture,
        &[source; 3],
        Some(VmafPixelFormat::Yuv444p),
        10,
    )
    .unwrap();
    let first: Vec<_> = [0x123u16, 0x234].into_iter().flat_map(u16::to_ne_bytes).collect();
    let second: Vec<_> = [0x345u16, 0x156].into_iter().flat_map(u16::to_ne_bytes).collect();
    for buffer in buffers {
        assert_eq!(&buffer[..4], first);
        assert_eq!(&buffer[8..12], second);
        assert_eq!(&buffer[4..8], &[0xa5; 4]);
        assert_eq!(&buffer[12..], &[0xa5; 4]);
    }
}

#[test]
fn bottom_up_u16_fill_reverses_visible_rows_and_preserves_padding() {
    let words = [1u16, 2, 0, 0, 3, 4, 0, 0];
    let source = PlaneSource::from_u16(&words, 2, 2, -8, 8).unwrap();
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 16]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    VmafScorer::fill_picture(
        &mut picture,
        &[source; 3],
        Some(VmafPixelFormat::Yuv444p),
        16,
    )
    .unwrap();
    for buffer in buffers {
        let first: Vec<_> = [3u16, 4].into_iter().flat_map(u16::to_ne_bytes).collect();
        let second: Vec<_> = [1u16, 2].into_iter().flat_map(u16::to_ne_bytes).collect();
        assert_eq!(&buffer[..4], first);
        assert_eq!(&buffer[8..12], second);
        assert_eq!(&buffer[4..8], &[0xa5; 4]);
        assert_eq!(&buffer[12..], &[0xa5; 4]);
    }
}

#[test]
fn foreign_plane_fill_reads_only_initialized_visible_bytes() {
    let mut foreign = [std::mem::MaybeUninit::<u8>::uninit(); 8];
    for (offset, byte) in [(0, 1), (1, 2), (4, 3), (5, 4)] {
        foreign[offset].write(byte);
    }
    // SAFETY: real 8-byte allocation outlives the view, and only the four
    // visible bytes are initialized. No slice over uninitialized gaps exists.
    let source =
        unsafe { PlaneSource::from_raw_parts(foreign.as_ptr().cast(), 8, 2, 2, -4, 4, 1) }.unwrap();
    let mut buffers = std::array::from_fn(|_| vec![0xa5; 8]);
    let mut picture = host_picture(&mut buffers, 2, 2);
    VmafScorer::fill_picture(
        &mut picture,
        &[source; 3],
        Some(VmafPixelFormat::Yuv444p),
        8,
    )
    .unwrap();
    for buffer in buffers {
        assert_eq!(buffer, [3, 4, 0xa5, 0xa5, 1, 2, 0xa5, 0xa5]);
    }
}

#[test]
fn pixel_format_mapping_matches_libvmaf() {
    let format = |chroma| VideoFormat {
        width: 64,
        height: 64,
        bit_depth: 10,
        chroma_sampling: chroma,
    };

    assert_eq!(
        format(ChromaSubsampling::Yuv420).pixel_format(),
        Some(VmafPixelFormat::Yuv420p)
    );
    assert_eq!(
        format(ChromaSubsampling::Yuv422).pixel_format(),
        Some(VmafPixelFormat::Yuv422p)
    );
    assert_eq!(
        format(ChromaSubsampling::Yuv444).pixel_format(),
        Some(VmafPixelFormat::Yuv444p)
    );
    assert_eq!(format(ChromaSubsampling::Monochrome).pixel_format(), None);
}

#[test]
fn validate_rejects_unsupported_bit_depths() {
    for bit_depth in [0, 1, 9, 11, 13, 14, 15, 32] {
        let format = VideoFormat {
            width: 64,
            height: 64,
            bit_depth,
            chroma_sampling: ChromaSubsampling::Yuv420,
        };
        assert!(matches!(
            format.validate(),
            Err(VmafError::UnsupportedFormat { .. })
        ));
    }
}

#[test]
fn validate_accepts_every_supported_bit_depth() {
    for bit_depth in [8, 10, 12, 16] {
        let format = VideoFormat {
            width: 64,
            height: 64,
            bit_depth,
            chroma_sampling: ChromaSubsampling::Yuv420,
        };
        assert!(format.validate().is_ok());
    }
}

#[test]
fn pooling_of_an_empty_slice_is_an_error() {
    assert!(matches!(
        VmafScorer::pool(&[], PoolMethod::Mean),
        Err(VmafError::InvalidConfiguration { .. })
    ));
}

#[test]
fn pooling_matches_hand_computed_values() {
    let scores: Vec<FrameScore> = [10.0, 20.0, 30.0]
        .into_iter()
        .map(|score| FrameScore {
            score,
            features: Vec::new(),
        })
        .collect();

    assert_eq!(VmafScorer::pool(&scores, PoolMethod::Mean).unwrap(), 20.0);
    assert_eq!(VmafScorer::pool(&scores, PoolMethod::Min).unwrap(), 10.0);
    assert_eq!(VmafScorer::pool(&scores, PoolMethod::Max).unwrap(), 30.0);
    // n / sum(1/x_i) = 3 / (1/10 + 1/20 + 1/30) = 3 / (11/60) = 180/11.
    let harmonic = VmafScorer::pool(&scores, PoolMethod::HarmonicMean).unwrap();
    assert!(
        (harmonic - 180.0 / 11.0).abs() < 1e-9,
        "harmonic mean was {harmonic}"
    );
}
