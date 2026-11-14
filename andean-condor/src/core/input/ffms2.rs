//! Native FFMS2 input: construction, format filtering, and y4m serialisation.

use std::{
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use av_decoders::{Decoder, Ffms2Decoder};
use av_format::rational::Rational64;

use crate::{
    core::input::{
        IndexProgress, Input, InputError, RawFrame, RawPlane,
        clip_info::ClipInfo,
        color_range::ColorRange,
        ffms2_filter::{Ffms2Filter, ffms2_pixel_format},
        frame_feed,
        pixel_format::PixelFormat,
    },
    models::input::{ImportMethod, Input as InputModel},
};

/// Precedes every frame's planes in a y4m stream.
const FRAME_HEADER: &[u8] = b"FRAME\n";

/// Applies each filter in turn. A filter's unset fields inherit from the
/// previous one, and the first filter's from the decoded stream. Applied before
/// any frame is read, so the decoder's `VideoDetails` describes what it
/// produces.
#[inline]
fn apply_filters(decoder: &mut Ffms2Decoder, filters: &[Ffms2Filter]) -> Result<()> {
    for filter in filters {
        let Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = filter;
        let current = decoder.video_details;

        let bit_depth = bit_depth.map_or(current.bit_depth, usize::from);
        let chroma = chroma.map_or(current.chroma_sampling, |chroma| {
            chroma.to_chroma_subsampling()
        });
        let width = width.map(usize::try_from).transpose()?.unwrap_or(current.width);
        let height = height.map(usize::try_from).transpose()?.unwrap_or(current.height);

        set_output_format(decoder, width, height, bit_depth, chroma)?;
    }
    Ok(())
}

fn validate_output_dimensions(width: usize, height: usize) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0 && i32::try_from(width).is_ok() && i32::try_from(height).is_ok(),
        "FFMS2 output dimensions {width}x{height} must be positive and fit i32"
    );
    Ok(())
}

/// Guard the dependency's narrowing casts and verify what FFMS2 actually chose.
fn set_output_format(
    decoder: &mut Ffms2Decoder,
    width: usize,
    height: usize,
    bit_depth: usize,
    chroma: av_decoders::v_frame::chroma::ChromaSubsampling,
) -> Result<()> {
    validate_output_dimensions(width, height)?;
    let mut expected = decoder.video_details;
    expected.width = width;
    expected.height = height;
    expected.bit_depth = bit_depth;
    expected.chroma_sampling = chroma;
    frame_feed::output_format_id(&expected)?;
    decoder.set_output_format(width, height, u8::try_from(bit_depth)?, chroma)?;
    // SAFETY: the decoder exclusively owns the source; validation reads only
    // the header, never planes, and the expected configuration is a snapshot.
    let frame = unsafe { frame_feed::ffms2_header_at(decoder.video_source, 0)? };
    frame_feed::validate_metadata(frame, &expected, 0)
}

/// av-decoders erases endianness from VideoDetails. Select LE once for initial
/// BE; the native setter also fixes output geometry and may resize later source
/// changes per frame. Rust does not request conversion again on each read.
fn normalize_output(decoder: &mut Ffms2Decoder) -> Result<()> {
    let details = decoder.video_details;
    frame_feed::output_format_id(&details)?;
    // SAFETY: the decoder owns a live, exclusive source. Only the header is read.
    let frame = unsafe { frame_feed::ffms2_header_at(decoder.video_source, 0)? };
    if details.bit_depth > 8 {
        let name = ffms2_pixel_format(details.bit_depth, details.chroma_sampling)
            .expect("output format checked above")
            .to_pix_fmt_string();
        let be_name = std::ffi::CString::new(format!("{}be", name.trim_end_matches("le")))?;
        // SAFETY: valid C string; lookup does not access source or planes.
        let be_format = unsafe { ffms2_sys::FFMS_GetPixFmt(be_name.as_ptr()) };
        if be_format >= 0 && frame.ConvertedPixelFormat == be_format {
            return set_output_format(
                decoder,
                details.width,
                details.height,
                details.bit_depth,
                details.chroma_sampling,
            );
        }
    }
    frame_feed::validate_metadata(frame, &details, 0)
}

/// Validates that a native input path names an existing, non-script video file.
pub fn validate(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.exists(),
        InputError::VideoFileNotFound(path.to_owned())
    );
    if let Some(ext) = path.extension() {
        anyhow::ensure!(
            ext != "vpy" && ext != "py",
            InputError::NotAVideoFile(path.to_owned())
        );
    }
    Ok(())
}

/// Opens a native FFMS2 input and applies its filters.
pub fn from_video(data: &InputModel) -> Result<Input> {
    let InputModel::Video {
        path,
        import_method,
        filters,
    } = data
    else {
        panic!("expected `Input::Video`");
    };

    Input::validate(data)?;
    match import_method {
        // ImportMethod::FFmpeg {} => {
        //     unimplemented!();
        // },
        ImportMethod::FFMS2 { index, cache_path } => {
            let mut ffms2_decoder = match cache_path {
                Some(cache_path) => Ffms2Decoder::with_index(path, cache_path, *index, None)?,
                // No cache path: FFMS2's own default, `{source}.ffindex`.
                None => Ffms2Decoder::new(path, *index)?,
            };

            // Captured before any filter runs, so a filter's unset fields
            // resolve against the source rather than the filtered
            // format.
            let source_details = ffms2_decoder.video_details;

            apply_filters(&mut ffms2_decoder, filters)?;
            normalize_output(&mut ffms2_decoder)?;

            let decoder =
                Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(ffms2_decoder))?;

            Ok(Input::Video {
                path: path.clone(),
                import_method: import_method.clone(),
                filters: filters.clone(),
                source_details,
                decoder,
                clip_info: None,
            })
        },
    }
}

/// Describes a native input's clip from the decoder's own `VideoDetails`,
/// which FFMS2 populates with the source's colour metadata.
pub fn clip_info(decoder: &Decoder) -> Result<ClipInfo> {
    let details = decoder.get_video_details();
    Ok(ClipInfo {
        format_info: PixelFormat::FFmpeg {
            format: ffms2_pixel_format(details.bit_depth, details.chroma_sampling).ok_or_else(
                || {
                    anyhow::anyhow!(
                        "FFMS2 cannot have produced {}-bit {:?}",
                        details.bit_depth,
                        details.chroma_sampling
                    )
                },
            )?,
        },
        frame_rate: Rational64::new(
            i64::from(*details.frame_rate.numer()),
            i64::from(*details.frame_rate.denom()),
        ),
        color_range: details.color_range.and_then(map_color_range),
        color_primaries: details.color_primaries,
        color_transfer_characteristic: details.color_transfer_characteristic,
        color_space: details.color_space,
        chroma_location: details.chroma_location,
        mastering_display: details.mastering_display,
        content_light_level: details.content_light_level,
        resolution: (details.width as u32, details.height as u32),
        num_frames: details
            .total_frames
            .ok_or_else(|| anyhow::anyhow!("FFMS2 decoder did not report a frame count"))?,
    })
}

/// Maps av-decoders' colour range onto the local one
#[inline]
const fn map_color_range(color_range: av_decoders::ColorRange) -> Option<ColorRange> {
    match color_range {
        av_decoders::ColorRange::Full => Some(ColorRange::Full),
        av_decoders::ColorRange::Limited => Some(ColorRange::Limited),
        av_decoders::ColorRange::Unspecified => None,
    }
}

/// Ensures an FFMS2 index exists for `path`.
/// Returns `true` when an index was built (false if a valid one was already
/// cached).
///
/// `cache_path` selects where the index lives, defaulting to
/// `{path}.ffindex`. Progress is FFMS2's own whole-percentage report, mapped.
pub fn index_video_with_progress(
    path: &Path,
    cache_path: Option<&Path>,
    mut progress: Option<IndexProgress<'_>>,
    cancelled: Option<&AtomicBool>,
) -> Result<bool> {
    // FFMS2 reports whole percentages
    let mut progress_fn = |percentage: u8| {
        if let Some(progress) = progress.as_mut() {
            (**progress)(u64::from(percentage), 100);
        }
    };
    let cancelled_fn = || cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed));

    Ok(Ffms2Decoder::build_index(
        path,
        cache_path,
        Some(&mut progress_fn),
        Some(&cancelled_fn),
    )?)
}

pub fn index_video_cancel(
    path: &Path,
    cache_path: Option<&Path>,
    mut progress: Option<IndexProgress<'_>>,
    cancellation: &crate::utils::cancellation::Cancellation,
) -> Result<bool> {
    cancellation.check_admission()?;
    let mut default_cache = path.as_os_str().to_os_string();
    default_cache.push(".ffindex");
    let default_cache = std::path::PathBuf::from(default_cache);
    let mut partial = crate::core::process::InterruptedOutput::new(
        cache_path.unwrap_or(&default_cache),
        cancellation,
    );
    let mut report = |percentage: u8| {
        if let Some(progress) = progress.as_mut() {
            (**progress)(u64::from(percentage), 100);
        }
    };
    let cancelled = || cancellation.is_forced();
    let result = Ffms2Decoder::build_index(path, cache_path, Some(&mut report), Some(&cancelled));
    cancellation.check_forced()?;
    let result = result?;
    partial.complete()?;
    Ok(result)
}

/// Writes a single decoded frame to `stream` as a complete y4m frame.
pub fn y4m_frame(decoder: &mut Decoder, index: usize, stream: &mut Vec<u8>) -> Result<()> {
    pack_frame(decoder, index, stream)
}

/// Streams `frame_indices` as y4m frames through `emit`, in order. Each frame
/// is emitted before the next is decoded, so `emit` bounds resident frames.
pub fn y4m_frames(
    decoder: &mut Decoder,
    frame_indices: &[usize],
    mut emit: impl FnMut(Vec<u8>) -> Result<()>,
) -> Result<()> {
    for &index in frame_indices {
        let mut buffer = Vec::new();
        pack_frame(decoder, index, &mut buffer)?;
        emit(buffer)?;
    }
    Ok(())
}

/// Packs `index` into `out` as a complete y4m frame — header plus plane data —
/// straight from FFMS2's own planes, without building an av-decoders `Frame`.
fn pack_frame(decoder: &mut Decoder, index: usize, out: &mut Vec<u8>) -> Result<()> {
    let ffms2 = decoder
        .get_ffms2_impl()
        .ok_or_else(|| anyhow::anyhow!("native input does not decode with FFMS2"))?;
    let video_source = ffms2.video_source;
    let details = ffms2.video_details;
    // SAFETY: `video_source` belongs to `decoder`, which this call borrows, and
    // `frame` is local — its planes are read into `out` here and the view
    // cannot outlive this call, so a later `FFMS_GetFrame` cannot overlap it.
    let frame = unsafe { frame_feed::ffms2_frame_at(video_source, &details, index)? };
    out.write_all(FRAME_HEADER)?;
    write_raw_frame(out, &frame);
    Ok(())
}

/// Appends a raw frame's planes to `stream` in Y, U, V order, visible rows
/// only — y4m carries no stride padding. Monochrome frames carry luma alone.
fn write_raw_frame(stream: &mut Vec<u8>, frame: &RawFrame<'_>) {
    write_raw_plane(stream, &frame.luma);
    if let Some((cb, cr)) = &frame.chroma {
        write_raw_plane(stream, cb);
        write_raw_plane(stream, cr);
    }
}

/// Appends one raw plane's visible rows to `stream`.
fn write_raw_plane(stream: &mut Vec<u8>, plane: &RawPlane<'_>) {
    for row in 0..plane.height() {
        stream.extend_from_slice(plane.row(row));
    }
}

#[cfg(test)]
mod tests {
    use av_decoders::v_frame::{
        chroma::ChromaSubsampling, frame::Frame, pixel::Pixel, plane::Plane,
    };

    use super::*;

    fn run(command: &mut std::process::Command) -> Vec<u8> {
        // Spool both streams so a full pipe cannot deadlock the watchdog.
        let stdout = tempfile::NamedTempFile::new().expect("child stdout");
        let stderr = tempfile::NamedTempFile::new().expect("child stderr");
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(stdout.reopen().expect("stdout handle"))
            .stderr(stderr.reopen().expect("stderr handle"))
            .spawn()
            .expect("ffmpeg/ffprobe required for native frame-layout fixtures");
        let start = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("child status") {
                break status;
            }
            if start.elapsed() > std::time::Duration::from_secs(30) {
                child.kill().expect("kill timed out child");
                child.wait().expect("reap timed out child");
                panic!("frame-layout native fixture command timed out: {command:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "{command:?}: {}",
            std::fs::read_to_string(stderr.path()).expect("child diagnostic")
        );
        std::fs::read(stdout.path()).expect("child output")
    }

    fn ffmpeg(args: &[&str]) -> Vec<u8> {
        run(std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args))
    }

    #[test]
    fn initial_big_endian_source_normalizes_to_le() {
        let directory = tempfile::tempdir().expect("BE fixture");
        let raw_path = directory.path().join("source.raw");
        let samples: Vec<u16> = (0..512).map(|i| 0x0123 + i % 113).collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|value| value.to_be_bytes()).collect();
        std::fs::write(&raw_path, bytes).expect("BE raw samples");
        let nut = ffmpeg(&[
            "-f",
            "rawvideo",
            "-pixel_format",
            "gray10be",
            "-video_size",
            "16x16",
            "-framerate",
            "24",
            "-i",
            raw_path.to_str().expect("fixture path"),
            "-frames:v",
            "2",
            "-c:v",
            "copy",
            "-f",
            "nut",
            "pipe:1",
        ]);
        let path = directory.path().join("source.nut");
        std::fs::write(&path, nut).expect("genuine BE NUT");
        let native = Ffms2Decoder::new(&path, None).expect("FFMS2 supports BE gray10 NUT");
        // SAFETY: source is live and exclusive; only header metadata is read.
        let header =
            unsafe { frame_feed::ffms2_header_at(native.video_source, 0) }.expect("BE header");
        // SAFETY: constant format name; no plane access.
        let be = unsafe { ffms2_sys::FFMS_GetPixFmt(c"gray10be".as_ptr()) };
        assert_eq!(
            header.EncodedPixelFormat, be,
            "file really stores BE, not FFV1's native endian"
        );
        assert_eq!(header.ConvertedPixelFormat, be, "initial FFMS output is BE");
        drop(native);
        let mut input = from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        })
        .expect("normalizes initial BE source");
        let Input::Video {
            decoder,
            source_details,
            ..
        } = &mut input
        else {
            unreachable!()
        };
        assert_eq!(source_details.bit_depth, 10);
        for index in 0..2 {
            let mut packed = Vec::new();
            pack_frame(decoder, index, &mut packed).expect("LE native packer");
            let expected: Vec<u8> = samples[index * 256..(index + 1) * 256]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            assert_eq!(&packed[..6], b"FRAME\n");
            assert_eq!(packed[6..], expected);
        }
        let native = decoder.get_ffms2_impl().expect("native source");
        // SAFETY: valid exclusive source and a terminated list of runtime IDs.
        // Passing null ErrorInfo is supported; the return code is checked.
        let status = unsafe {
            ffms2_sys::FFMS_SetOutputFormatV2(
                native.video_source,
                [be, -1].as_ptr(),
                16,
                16,
                ffms2_sys::FFMS_Resizers::FFMS_RESIZER_BICUBIC as i32,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, 0, "force unexpected BE after construction");
        let mut packed = vec![77];
        let error = pack_frame(decoder, 1, &mut packed).expect_err("never read unexpected BE");
        assert!(error.to_string().contains("frame 1"));
        assert_eq!(packed, [77], "failed preflight emits no partial frame");

        // Intentional scaling uses converted dimensions, not Encoded*, and
        // does not rewrite the original source details retained for filters.
        let native = decoder.get_ffms2_impl().expect("native source");
        set_output_format(native, 8, 8, 12, ChromaSubsampling::Monochrome)
            .expect("intentional resize");
        // SAFETY: live exclusive source, header-only read.
        let header =
            unsafe { frame_feed::ffms2_header_at(native.video_source, 0) }.expect("scaled header");
        assert_eq!((header.EncodedWidth, header.EncodedHeight), (16, 16));
        assert_eq!((header.ScaledWidth, header.ScaledHeight), (8, 8));
        assert_eq!(
            (
                source_details.width,
                source_details.height,
                source_details.bit_depth
            ),
            (16, 16, 10)
        );
        let mut packed = Vec::new();
        pack_frame(decoder, 0, &mut packed).expect("converted shape accepted");
        assert_eq!(packed.len(), 6 + 8 * 8 * 2);
    }

    #[test]
    fn ffms2_output_dimensions_reject_zero_and_i32_overflow() {
        for (width, height) in [
            (0, 16),
            (16, 0),
            (i32::MAX as usize + 1, 16),
            (16, u32::MAX as usize),
            (usize::MAX, 16),
        ] {
            assert!(validate_output_dimensions(width, height).is_err());
        }
        assert!(validate_output_dimensions(1, 1).is_ok());
        assert!(validate_output_dimensions(i32::MAX as usize, i32::MAX as usize).is_ok());
    }

    fn check_native_result(
        index: usize,
        converted: (i32, i32),
        result: Result<()>,
        packed: &[u8],
    ) -> bool {
        if converted == (32, 1080) {
            result.expect("unchanged converted output must pack");
            assert_eq!(&packed[..6], b"FRAME\n");
            assert_eq!(packed.len(), 6 + 32 * 1080 * 3 / 2);
            false
        } else {
            let message =
                result.expect_err("changed converted output must be rejected").to_string();
            assert!(message.contains(&format!("frame {index}")), "{message}");
            assert!(message.contains("expected 32x1080"), "{message}");
            assert!(
                message.contains(&format!("got {}x{}", converted.0, converted.1)),
                "{message}"
            );
            assert!(packed.is_empty(), "reject before writing any planes");
            true
        }
    }

    // These exercise the native test's result oracle without a decoder or raw
    // views. Never remove native validation to manufacture an over-reading RED.
    #[test]
    #[should_panic(expected = "changed converted output must be rejected")]
    fn native_oracle_rejects_false_success_on_changed_header() {
        check_native_result(1, (32, 800), Ok(()), &vec![0; 6 + 32 * 1080 * 3 / 2]);
    }

    #[test]
    #[should_panic(expected = "unchanged converted output must pack")]
    fn native_oracle_rejects_unexercised_decode_error() {
        check_native_result(
            0,
            (32, 1080),
            Err(anyhow::anyhow!(
                "FFMS2 could not decode frame 0: unavailable"
            )),
            &[],
        );
    }

    #[test]
    fn native_changing_resolution_capability() {
        let directory = tempfile::tempdir().expect("changing resolution fixture");
        let mut encoded = Vec::new();
        for source in ["color=c=black:s=32x1080:r=24", "color=c=white:s=32x800:r=24"] {
            encoded.extend(ffmpeg(&[
                "-f",
                "lavfi",
                "-i",
                source,
                "-frames:v",
                "2",
                "-c:v",
                "mpeg2video",
                "-threads",
                "1",
                "-g",
                "1",
                "-bf",
                "0",
                "-f",
                "mpeg2video",
                "pipe:1",
            ]));
        }
        let path = directory.path().join("changing.m2v");
        std::fs::write(&path, encoded).expect("concatenated encoded segments");
        let probe = run(std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "frame=width,height", "-of", "json"])
            .arg(&path));
        let probe: serde_json::Value = serde_json::from_slice(&probe).expect("probe frames");
        let shapes: Vec<_> = probe["frames"]
            .as_array()
            .expect("decoded frames")
            .iter()
            .map(|frame| {
                (
                    frame["width"].as_u64().expect("width"),
                    frame["height"].as_u64().expect("height"),
                )
            })
            .collect();
        // MPEG2 decoders may discard the first segment's delayed final frame
        // at a new sequence header; either way both actual shapes must decode.
        assert!(
            shapes == [(32, 1080), (32, 800), (32, 800)]
                || shapes == [(32, 1080), (32, 1080), (32, 800), (32, 800)],
            "genuine changing bitstream: {shapes:?}"
        );
        eprintln!("frame-layout native capability: ffprobe decoded {shapes:?}");
        let native = match Ffms2Decoder::new(&path, None) {
            Ok(native) => native,
            Err(error) => {
                eprintln!(
                    "frame-layout native capability: FFMS2 refused changing MPEG2 index/source: {error}"
                );
                return;
            },
        };
        let indexed = native.video_details.total_frames.expect("FFMS2 indexed frame count");
        assert!(indexed > 0, "fixture must have a baseline frame");
        assert_eq!(
            (native.video_details.width, native.video_details.height),
            (32, 1080)
        );
        let mut decoder = Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(native))
            .expect("native decoder");
        let mut baseline = Vec::new();
        let result = pack_frame(&mut decoder, 0, &mut baseline);
        check_native_result(0, (32, 1080), result, &baseline);
        assert!(
            baseline[6..6 + 32 * 1080].iter().all(|&value| value == 16),
            "black baseline luma"
        );
        assert!(
            baseline[6 + 32 * 1080..].iter().all(|&value| value == 128),
            "neutral baseline chroma"
        );
        eprintln!(
            "frame-layout native capability: baseline frame 0 packed correctly; {indexed} frames indexed"
        );
        let mut exposed = 0;
        let mut mismatches = 0;
        let mut decode_errors = 0;
        let mut encoded_change = None;
        for index in 1..indexed {
            let native = decoder.get_ffms2_impl().expect("native source");
            // SAFETY: source is live and exclusive. Copy header geometry only,
            // then retire the header before the packer's next native read.
            let header = match unsafe { frame_feed::ffms2_header_at(native.video_source, index) } {
                Ok(header) => header,
                Err(error) => {
                    decode_errors += 1;
                    eprintln!(
                        "frame-layout native capability: header decode refused frame {index}: {error}"
                    );
                    continue;
                },
            };
            let encoded = (header.EncodedWidth, header.EncodedHeight);
            // Independent oracle: only the native no-scaler sentinel uses the
            // encoded shape. An encoded-only change is not converted drift.
            let converted = match (header.ScaledWidth, header.ScaledHeight) {
                (-1, -1) => encoded,
                shape => shape,
            };
            assert!(matches!(encoded, (32, 1080) | (32, 800)), "{encoded:?}");
            assert!(matches!(converted, (32, 1080) | (32, 800)), "{converted:?}");
            if encoded != (32, 1080) {
                encoded_change.get_or_insert(index);
            }
            exposed += usize::from(converted != (32, 1080));
            let mut packed = Vec::new();
            let result = pack_frame(&mut decoder, index, &mut packed);
            mismatches += usize::from(check_native_result(index, converted, result, &packed));
        }
        if exposed > 0 {
            assert!(
                mismatches > 0,
                "exposed converted changes must exercise rejection"
            );
            assert_eq!(
                mismatches, exposed,
                "every exposed converted change must reject"
            );
            eprintln!(
                "frame-layout native capability: {mismatches}/{exposed} exposed converted changes rejected without bytes; {decode_errors} header decode errors"
            );
        } else {
            eprintln!(
                "frame-layout native capability: NO changing-frame rejection exercised; {indexed} indexed, {decode_errors} later header decode errors, changed encoded frame {encoded_change:?}; decoder refused changes or kept converted output fixed"
            );
        }
        // Explicit output conversion is allowed to keep output fixed even when
        // the encoded shape changes. This must not be rejected as source drift.
        if let Some(index) = encoded_change {
            let native = decoder.get_ffms2_impl().expect("native source");
            set_output_format(native, 32, 1080, 8, ChromaSubsampling::Yuv420)
                .expect("fixed output");
            // SAFETY: live exclusive source; inspect header only before packing.
            let header = unsafe { frame_feed::ffms2_header_at(native.video_source, index) }
                .expect("fixed output header");
            assert_eq!((header.EncodedWidth, header.EncodedHeight), (32, 800));
            assert_eq!((header.ScaledWidth, header.ScaledHeight), (32, 1080));
            let mut packed = Vec::new();
            pack_frame(&mut decoder, index, &mut packed)
                .expect("FFMS2 resizes changed encoded shape");
            assert_eq!(packed.len(), 6 + 32 * 1080 * 3 / 2);
            eprintln!(
                "frame-layout native capability: fixed output accepted actual changed encoded frame {index}"
            );
        } else {
            eprintln!(
                "frame-layout native capability: no decoded changed encoded frame available for fixed-output control"
            );
        }
    }

    /// A pixel type the Frame-era writer serialises. `Pixel` is sealed to
    /// `u8` and `u16`, so these cover every frame the native decoder produced.
    trait Sample: Pixel + Sized {
        /// Appends one visible row of this plane to `stream`.
        fn write_row(stream: &mut Vec<u8>, row: &[Self]);

        /// Builds a pixel from a 16-bit test value, for the byte-equivalence
        /// tests.
        fn from_test(value: u16) -> Self;
    }

    impl Sample for u8 {
        fn write_row(stream: &mut Vec<u8>, row: &[u8]) {
            stream.extend_from_slice(row);
        }

        fn from_test(value: u16) -> Self {
            value as Self
        }
    }

    impl Sample for u16 {
        fn write_row(stream: &mut Vec<u8>, row: &[u16]) {
            if cfg!(target_endian = "little") {
                let bytes = unsafe {
                    // SAFETY: `row` is a live `[u16]`, so `size_of_val(row)`
                    // bytes are initialised and readable
                    // from `row.as_ptr()`, and `u8`
                    // has an alignment of 1, so the cast is properly aligned.
                    // The slice is only read and does not
                    // outlive `row`.
                    std::slice::from_raw_parts(
                        row.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(row),
                    )
                };
                stream.extend_from_slice(bytes);
            } else {
                // y4m stores samples little-endian.
                stream.reserve(std::mem::size_of_val(row));
                for sample in row {
                    stream.extend_from_slice(&sample.to_le_bytes());
                }
            }
        }

        fn from_test(value: u16) -> Self {
            value
        }
    }

    /// The Frame-era writer, kept as the oracle the raw packer is proven
    /// against: appends a decoded frame's planes as y4m plane data in Y, U, V
    /// order, skipping padding. Bulk row copies rather than `Plane::byte_data`,
    /// which yields one byte at a time.
    fn write_y4m_frame<T: Sample>(stream: &mut Vec<u8>, frame: &Frame<T>) {
        let planes = [Some(&frame.y_plane), frame.u_plane.as_ref(), frame.v_plane.as_ref()];
        for plane in planes.into_iter().flatten() {
            for row in plane.rows() {
                T::write_row(stream, row);
            }
        }
    }

    /// Every visible pixel distinct, padding left at zero, so a wrong row
    /// order, dropped plane, or serialised padding shows up as a mismatch.
    fn sample_frame<T: Sample>(width: usize, height: usize, chroma: ChromaSubsampling) -> Frame<T> {
        let mut frame = av_decoders::v_frame::frame::FrameBuilder::new(
            width,
            height,
            chroma,
            if size_of::<T>() == 1 { 8 } else { 10 },
        )
        .luma_padding_left(8)
        .luma_padding_right(8)
        .luma_padding_top(8)
        .luma_padding_bottom(8)
        .build()
        .expect("valid frame geometry");

        let mut next = 1u16;
        for plane in [Some(&mut frame.y_plane), frame.u_plane.as_mut(), frame.v_plane.as_mut()]
            .into_iter()
            .flatten()
        {
            for row in plane.rows_mut() {
                for sample in row {
                    *sample = T::from_test(next);
                    next = next.wrapping_add(1);
                }
            }
        }
        frame
    }

    /// Bulk row copies are only safe if they match `Plane::byte_data` exactly.
    #[test]
    fn ffms2_eight_bit_writer_matches_visible_frame_plane_bytes() {
        for chroma in [
            ChromaSubsampling::Yuv420,
            ChromaSubsampling::Yuv422,
            ChromaSubsampling::Yuv444,
            ChromaSubsampling::Monochrome,
        ] {
            let frame = sample_frame::<u8>(64, 32, chroma);
            let mut expected = Vec::new();
            expected.extend(frame.y_plane.byte_data());
            if let Some(plane) = &frame.u_plane {
                expected.extend(plane.byte_data());
            }
            if let Some(plane) = &frame.v_plane {
                expected.extend(plane.byte_data());
            }
            let mut actual = Vec::new();
            write_y4m_frame(&mut actual, &frame);
            assert_eq!(
                actual, expected,
                "8-bit writer differs from byte_data for {chroma:?}"
            );
        }
    }

    /// As above for high bit depth, where y4m requires little-endian samples
    /// regardless of host byte order.
    #[test]
    fn ffms2_high_depth_writer_matches_little_endian_plane_bytes() {
        for chroma in [
            ChromaSubsampling::Yuv420,
            ChromaSubsampling::Yuv444,
            ChromaSubsampling::Monochrome,
        ] {
            let frame = sample_frame::<u16>(64, 32, chroma);
            let mut expected = Vec::new();
            expected.extend(frame.y_plane.byte_data());
            if let Some(plane) = &frame.u_plane {
                expected.extend(plane.byte_data());
            }
            if let Some(plane) = &frame.v_plane {
                expected.extend(plane.byte_data());
            }
            let mut actual = Vec::new();
            write_y4m_frame(&mut actual, &frame);
            assert_eq!(
                actual, expected,
                "16-bit writer differs from byte_data for {chroma:?}"
            );
            // Samples really did come out little-endian.
            assert_eq!(&actual[..2], &[1, 0]);
        }
    }

    /// Re-lays a frame's visible pixels into a stride-padded decoder buffer,
    /// the layout `ffms2_frame_at` reads. Stride deliberately differs from
    /// the `Frame`'s own, so padding mistakes change the packed bytes.
    fn decoder_layout<T: Pixel>(plane: &Plane<T>, stride: usize) -> (Vec<u8>, usize) {
        let bytes = size_of::<T>();
        let stride_bytes = stride * bytes;
        let mut data = vec![0u8; stride_bytes * plane.height()];
        for (row_index, row) in plane.rows().enumerate() {
            for (col, value) in row.iter().enumerate() {
                let value = Into::<u16>::into(*value);
                let at = row_index * stride_bytes + col * bytes;
                if bytes == 1 {
                    data[at] = value as u8;
                } else {
                    data[at..at + bytes].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        (data, stride_bytes)
    }

    fn compare_writers<T: Sample>(width: usize, height: usize, chroma: ChromaSubsampling) {
        let format = format!("{chroma:?} {width}x{height}");
        let frame = sample_frame::<T>(width, height, chroma);
        let mut expected = Vec::new();
        write_y4m_frame(&mut expected, &frame);

        let bytes = size_of::<T>();
        let bit_depth = if bytes == 1 { 8 } else { 10 };
        let (luma_data, luma_stride) = decoder_layout(&frame.y_plane, width + 6);
        let luma = RawPlane::new(
            &luma_data,
            luma_stride,
            frame.y_plane.width(),
            frame.y_plane.height(),
            bytes,
        )
        .unwrap_or_else(|| panic!("{format}: luma plane rejected"));
        let u_layout = frame.u_plane.as_ref().map(|u| decoder_layout(u, u.width() + 4));
        let v_layout = frame.v_plane.as_ref().map(|v| decoder_layout(v, v.width() + 4));
        let chroma_pair = match (
            frame.u_plane.as_ref(),
            frame.v_plane.as_ref(),
            &u_layout,
            &v_layout,
        ) {
            (Some(u), Some(v), Some((u_data, u_stride)), Some((v_data, v_stride))) => {
                let cb = RawPlane::new(u_data, *u_stride, u.width(), u.height(), bytes)
                    .unwrap_or_else(|| panic!("{format}: Cb plane rejected"));
                let cr = RawPlane::new(v_data, *v_stride, v.width(), v.height(), bytes)
                    .unwrap_or_else(|| panic!("{format}: Cr plane rejected"));
                Some((cb, cr))
            },
            _ => None,
        };

        let raw = RawFrame {
            luma,
            chroma: chroma_pair,
            bit_depth,
        };
        let mut actual = Vec::new();
        write_raw_frame(&mut actual, &raw);
        assert_eq!(actual, expected, "{format}");
    }

    /// The raw-plane writer (production `y4m_frames`) must emit exactly the
    /// bytes the `Frame`-based writer produced before the switch, across
    /// bit depths and chroma layouts including monochrome.
    #[test]
    fn raw_writer_matches_frame_writer() {
        compare_writers::<u8>(64, 32, ChromaSubsampling::Yuv420);
        compare_writers::<u8>(64, 32, ChromaSubsampling::Monochrome);
        compare_writers::<u16>(64, 32, ChromaSubsampling::Yuv444);
        compare_writers::<u16>(96, 48, ChromaSubsampling::Yuv422);
    }

    /// Hand-sized Y4M planes bypass the tile-only FrameBuilder restrictions.
    /// Floor-rounded native chroma loses edge samples and misaligns frame two.
    fn native_roundtrip(
        (width, height): (usize, usize),
        (chroma_width, chroma_height): (usize, usize),
        colorspace: &str,
        bit_depth: usize,
        payload: usize,
    ) {
        let bytes = if bit_depth == 8 { 1 } else { 2 };
        let geometry =
            [(width, height), (chroma_width, chroma_height), (chroma_width, chroma_height)];
        let frames: Vec<[Vec<u8>; 3]> = (0..2)
            .map(|index| {
                std::array::from_fn(|plane| {
                    let (w, h) = geometry[plane];
                    let mut data = Vec::with_capacity(w * h * bytes);
                    for row in 0..h {
                        for col in 0..w {
                            // Distinguish U/V, both frames, final column and final row;
                            // high-depth samples also require a nonzero high byte.
                            let value = 16
                                + plane * 40
                                + index * 9
                                + row % 7
                                + col % 11
                                + usize::from(col == w - 1) * 31
                                + usize::from(row == h - 1) * 67
                                + usize::from(bytes == 2) * 256;
                            data.extend_from_slice(&(value as u16).to_le_bytes()[..bytes]);
                        }
                    }
                    data
                })
            })
            .collect();
        let header = format!("YUV4MPEG2 W{width} H{height} F24:1 Ip A1:1 C{colorspace}\n");
        let mut source = header.as_bytes().to_vec();
        for planes in &frames {
            assert_eq!(planes.iter().map(Vec::len).sum::<usize>(), payload);
            source.extend_from_slice(b"FRAME\n");
            for plane in planes {
                source.extend_from_slice(plane);
            }
        }
        let directory = tempfile::tempdir().expect("native odd-size fixture");
        let path = directory.path().join("input.y4m");
        std::fs::write(&path, &source).expect("write valid two-frame Y4M");
        // Force FFMS2 rather than Decoder's extension-based Y4M shortcut.
        let native = Ffms2Decoder::new(&path, None).expect("real FFMS2 opens odd-size Y4M");
        let mut decoder = Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(native))
            .expect("native decoder");
        let mut packed = header.as_bytes().to_vec();
        for (index, expected) in frames.iter().enumerate() {
            let start = packed.len();
            pack_frame(&mut decoder, index, &mut packed).expect("production native packer");
            assert_eq!(packed.len() - start, payload + 6, "frame {index} payload");

            let native = decoder.get_ffms2_impl().expect("forced FFMS2 backend");
            // SAFETY: the decoder owns this live source; the view is consumed
            // before any subsequent read, with unmodified native metadata.
            let raw = unsafe {
                frame_feed::ffms2_frame_at(native.video_source, &native.video_details, index)
            }
            .expect("native raw planes");
            assert_eq!(raw.bit_depth, bit_depth);
            let (u, v) = raw.chroma.as_ref().expect("subsampled chroma");
            for ((plane, &(w, h)), expected) in
                [&raw.luma, u, v].into_iter().zip(&geometry).zip(expected)
            {
                assert_eq!((plane.width(), plane.height()), (w, h));
                assert_eq!(plane.bytes_per_sample(), bytes);
                for row in 0..h {
                    assert_eq!(
                        plane.row(row),
                        &expected[row * w * bytes..(row + 1) * w * bytes]
                    );
                }
                let last = &expected[expected.len() - bytes..];
                let last = if bytes == 1 {
                    u32::from(last[0])
                } else {
                    u32::from(u16::from_le_bytes([last[0], last[1]]))
                };
                assert_eq!(plane.sample(h - 1, w - 1), last);
            }
        }
        assert_eq!(packed.len(), header.len() + 2 * (payload + 6));
        let mut y4m = av_decoders::Y4mDecoder::new(packed.as_slice()).expect("packed Y4M header");
        assert_eq!((y4m.get_width(), y4m.get_height()), (width, height));
        assert_eq!(
            format!("{:?}", y4m.get_colorspace()),
            format!("C{colorspace}")
        );
        assert_eq!(y4m.get_bit_depth(), bit_depth);
        for expected in &frames {
            let frame = y4m.read_frame().expect("aligned complete Y4M frame");
            for (actual, expected) in
                [frame.get_y_plane(), frame.get_u_plane(), frame.get_v_plane()]
                    .into_iter()
                    .zip(expected)
            {
                assert_eq!(actual, expected);
            }
        }
        assert!(
            y4m.read_frame().is_err(),
            "exactly two frames, no trailing data"
        );
    }

    // Literal geometry/counts: 853*479 + 2*427*240 = 613547;
    // 853*480 + 2*427*240 = 614400; 853*479 + 2*427*479 = 817653.
    #[test]
    fn native_853x480_420() {
        native_roundtrip((853, 480), (427, 240), "420", 8, 614400);
    }

    #[test]
    fn native_853x479_420() {
        native_roundtrip((853, 479), (427, 240), "420", 8, 613547);
    }

    #[test]
    fn native_853x479_422() {
        native_roundtrip((853, 479), (427, 479), "422", 8, 817653);
    }

    #[test]
    fn native_853x479_420_high_depth() {
        native_roundtrip((853, 479), (427, 240), "420p10", 10, 1227094);
    }

    #[test]
    fn native_even_420_control() {
        native_roundtrip((64, 32), (32, 16), "420", 8, 3072);
    }
}
