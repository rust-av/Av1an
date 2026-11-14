//! Sequential raw-frame feeds over an [`Input`].
//!
//! A [`FrameFeed`] decodes `[first, end)` through whichever backend the input
//! uses and hands back [`RawFrame`] views of that backend's own plane memory,
//! skipping av-decoders' per-frame `Frame` allocation and plane copies.
//! Native input reads `FFMS_GetFrame`'s planes in place; VapourSynth input
//! keeps a bounded window of asynchronous frame requests in flight, so decode
//! overlaps consumption. Consumers tile or serialise what the feed returns —
//! the TransNetV2 scene detector builds its input tiles from it.

use std::{
    collections::BTreeMap,
    ffi::c_char,
    num::NonZeroUsize,
    sync::{Arc, Condvar, Mutex},
};

use anyhow::{Result, bail, ensure};
use av_decoders::VideoDetails;
use ffms2_sys::{FFMS_ErrorInfo, FFMS_Frame, FFMS_GetFrame, FFMS_VideoSource};
use vapoursynth::{frame::FrameRef, node::Node};

use super::{Input, ffms2_filter::ffms2_pixel_format, vapoursynth::output_node};

/// VapourSynth frame requests in flight: one per core keeps the decode
/// pipeline busy without letting resident frames grow without bound.
pub(super) fn request_window() -> usize {
    std::thread::available_parallelism().map_or(24, |threads| threads.get())
}

/// One plane's pixels read straight from a decoder: a byte buffer, the byte
/// distance between rows, and the visible geometry. Nothing is copied, so a
/// padded backend layout costs nothing to address.
pub struct RawPlane<'a> {
    data: &'a [u8],
    stride: usize,
    width: usize,
    height: usize,
    bytes: usize,
}

impl<'a> RawPlane<'a> {
    /// Views `data` as `width`×`height` samples of `bytes` bytes each, `stride`
    /// bytes apart. Returns `None` when `data` cannot hold the plane or `bytes`
    /// is a sample width this reader cannot address, so an unsupported backend
    /// layout fails loudly instead of being misread.
    #[must_use]
    pub(crate) fn new(
        data: &'a [u8],
        stride: usize,
        width: usize,
        height: usize,
        bytes: usize,
    ) -> Option<Self> {
        if !matches!(bytes, 1 | 2) || stride == 0 || width == 0 || height == 0 {
            return None;
        }
        let row = width.checked_mul(bytes)?;
        // A stride narrower than a row would make successive rows overlap.
        if stride < row {
            return None;
        }
        let required = stride.checked_mul(height - 1)?.checked_add(row)?;
        (data.len() >= required).then_some(Self {
            data,
            stride,
            width,
            height,
            bytes,
        })
    }

    /// The plane's visible width in samples.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The plane's visible height in rows.
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Bytes per sample: 1 for 8-bit planes, 2 for 9..=16-bit.
    #[inline]
    pub fn bytes_per_sample(&self) -> usize {
        self.bytes
    }

    /// The visible bytes of row `index`, without stride padding.
    #[inline]
    pub fn row(&self, index: usize) -> &'a [u8] {
        let at = index * self.stride;
        &self.data[at..at + self.width * self.bytes]
    }

    /// One visible sample as a `u32`, read little-endian.
    ///
    /// # Panics
    ///
    /// Panics when `row` or `col` is outside the plane.
    #[inline]
    pub fn sample(&self, row: usize, col: usize) -> u32 {
        let at = row * self.stride + col * self.bytes;
        if self.bytes == 1 {
            u32::from(self.data[at])
        } else {
            u32::from(u16::from_le_bytes([self.data[at], self.data[at + 1]]))
        }
    }
}

/// One decoded frame addressed in place: its luma plane, optional chroma
/// planes, and the sample depth they hold. The planes borrow the backend's
/// buffers, so a frame stays valid only until its feed produces the next one.
pub struct RawFrame<'a> {
    /// The luma plane.
    pub luma: RawPlane<'a>,
    /// Cb then Cr; `None` for monochrome.
    pub chroma: Option<(RawPlane<'a>, RawPlane<'a>)>,
    /// Bits per sample stored in the planes.
    pub bit_depth: usize,
}

/// Sequential raw frames for `[first, end)` of an [`Input`], produced by
/// whichever backend decodes it.
pub struct FrameFeed<'a> {
    kind: Kind<'a>,
}

enum Kind<'a> {
    Ffms2(Ffms2Feed),
    VapourSynth(VsFeed<'a>),
}

impl FrameFeed<'_> {
    /// Opens a feed over `[first, end)`. Both backends address frames by index,
    /// so a resumed run reads where the last one stopped rather than from the
    /// decoder's current position.
    pub(crate) fn new(input: &mut Input, first: usize, end: usize) -> Result<FrameFeed<'_>> {
        Ok(FrameFeed {
            kind: match input {
                Input::Video { decoder, .. } => {
                    let ffms2 = decoder.get_ffms2_impl().ok_or_else(|| {
                        anyhow::anyhow!("native input does not decode with FFMS2")
                    })?;
                    Kind::Ffms2(Ffms2Feed {
                        video_source: ffms2.video_source,
                        details: ffms2.video_details,
                        next: first,
                        end,
                    })
                },
                Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                    Kind::VapourSynth(VsFeed::new(output_node(decoder)?, first, end))
                },
            },
        })
    }

    /// The next frame, or `None` once `[first, end)` is exhausted. The frame
    /// borrows the feed, which is also what keeps the backend's planes alive
    /// that long: the next frame invalidates the previous one.
    #[inline]
    pub fn next_frame(&mut self) -> Result<Option<RawFrame<'_>>> {
        match &mut self.kind {
            Kind::Ffms2(feed) => feed.next_frame(),
            Kind::VapourSynth(feed) => feed.next_frame(),
        }
    }
}

/// Native FFMS2: each frame's planes are addressed where the decoder left them.
struct Ffms2Feed {
    video_source: *mut FFMS_VideoSource,
    details: VideoDetails,
    next: usize,
    end: usize,
}

impl Ffms2Feed {
    fn next_frame(&mut self) -> Result<Option<RawFrame<'_>>> {
        if self.next >= self.end {
            return Ok(None);
        }
        let index = self.next;
        // SAFETY: the feed's borrow of the input keeps `video_source` alive
        // and exclusive, so the frame's planes cannot be invalidated while
        // the caller still holds the returned frame.
        let raw = unsafe { ffms2_frame_at(self.video_source, &self.details, index)? };
        self.next = index + 1;
        Ok(Some(raw))
    }
}

/// Reads frame `index` from a native FFMS2 source, addressing its planes in
/// place instead of copying them into a `Frame`.
///
/// # Safety
///
/// `video_source` must be live and outlive the returned frame. No other read
/// or input/output format change may run on it while the view is in use.
/// FFMS2 must provide initialized plane allocations covering the validated
/// spans; a raw header cannot establish allocation extents or ownership.
pub(super) unsafe fn ffms2_frame_at<'a>(
    video_source: *mut FFMS_VideoSource,
    details: &VideoDetails,
    index: usize,
) -> Result<RawFrame<'a>> {
    // SAFETY: the caller keeps the source and all returned buffers live and
    // exclusive until this view is retired.
    let frame = unsafe { ffms2_header_at(video_source, index)? };
    // SAFETY: same source lifetime; all metadata/planes are preflighted before
    // any slice is constructed.
    unsafe { planes_of(frame, details, index) }
}

/// Reads only the native header, without borrowing any plane memory.
///
/// # Safety
/// The source must be live and exclusively accessed; the returned header is
/// invalidated by another read or any input/output format change.
pub(super) unsafe fn ffms2_header_at<'a>(
    video_source: *mut FFMS_VideoSource,
    index: usize,
) -> Result<&'a FFMS_Frame> {
    let mut error_buffer = [0 as c_char; 1024];
    let mut error = FFMS_ErrorInfo {
        ErrorType: 0,
        SubType: 0,
        BufferSize: i32::try_from(error_buffer.len()).expect("error buffer fits in i32"),
        Buffer: error_buffer.as_mut_ptr(),
    };
    let n = i32::try_from(index)
        .map_err(|_| anyhow::anyhow!("frame index {index} exceeds FFMS2's range"))?;
    // SAFETY: the caller guarantees `video_source` outlives this call, and
    // `error` points at a live buffer `BufferSize` bytes long.
    let frame = unsafe { FFMS_GetFrame(video_source, n, &mut error) };
    if frame.is_null() {
        bail!(
            "FFMS2 could not decode frame {index}: {}",
            error_message(&error)
        );
    }

    // SAFETY: the reference is non-null and stays valid until the next
    // `FFMS_GetFrame` on this source, which the safety contract forbids while
    // the returned frame is in use.
    Ok(unsafe { &*frame })
}

/// Runtime format IDs must come from the FFmpeg linked to FFMS2, not enums
/// compiled against another version. av-decoders 0.12 supports 8/10/12 only.
pub(super) fn output_format_id(details: &VideoDetails) -> Result<i32> {
    let format =
        ffms2_pixel_format(details.bit_depth, details.chroma_sampling).ok_or_else(|| {
            anyhow::anyhow!(
                "FFMS2 cannot output {}-bit {:?}",
                details.bit_depth,
                details.chroma_sampling
            )
        })?;
    let name = std::ffi::CString::new(format.to_pix_fmt_string())?;
    // SAFETY: `name` is a live NUL-terminated string; no frame memory is read.
    let id = unsafe { ffms2_sys::FFMS_GetPixFmt(name.as_ptr()) };
    ensure!(
        id >= 0,
        "FFMS2 does not support {}",
        format.to_pix_fmt_string()
    );
    Ok(id)
}

/// Checks actual converted output, not the encoded source or cached details.
pub(super) fn validate_metadata(
    frame: &FFMS_Frame,
    details: &VideoDetails,
    index: usize,
) -> Result<()> {
    let expected_format = output_format_id(details)?;
    // COMPAT: FFMS2 documents (-1, -1) when no scaler is configured. Only
    // that sentinel uses Encoded*, which then describes the actual Data.
    let (width, height) = match (frame.ScaledWidth, frame.ScaledHeight) {
        (-1, -1) => (frame.EncodedWidth, frame.EncodedHeight),
        dimensions => dimensions,
    };
    ensure!(
        width > 0
            && height > 0
            && i32::try_from(details.width).ok() == Some(width)
            && i32::try_from(details.height).ok() == Some(height)
            && frame.ConvertedPixelFormat == expected_format,
        "FFMS2 frame {index}: expected {}x{} {}-bit {:?} LE planar (format {expected_format}), got {width}x{height} format {} (scaled {}x{}, encoded {}x{})",
        details.width,
        details.height,
        details.bit_depth,
        details.chroma_sampling,
        frame.ConvertedPixelFormat,
        frame.ScaledWidth,
        frame.ScaledHeight,
        frame.EncodedWidth,
        frame.EncodedHeight,
    );
    Ok(())
}

/// The planes of one decoded FFMS2 frame, addressed in place.
///
/// # Safety
///
/// Every required non-null plane must point into one live, initialized
/// allocation containing at least its validated span. Header checks cannot
/// prove raw allocation extents. Buffers must remain immutable and live for
/// the returned lifetime (no read or format change on the same source).
unsafe fn planes_of<'a>(
    frame: &'a FFMS_Frame,
    details: &VideoDetails,
    index: usize,
) -> Result<RawFrame<'a>> {
    let (luma, chroma) = preflight_planes(frame, details, index)?;
    // SAFETY: ALL required headers/spans passed preflight, and the caller
    // guarantees the underlying allocations and lifetime, not just metadata.
    unsafe {
        Ok(RawFrame {
            luma: luma.view(),
            chroma: chroma.map(|(cb, cr)| (cb.view(), cr.view())),
            bit_depth: details.bit_depth,
        })
    }
}

struct PlaneLayout {
    data: *const u8,
    stride: usize,
    width: usize,
    height: usize,
    bytes: usize,
    len: usize,
}

impl PlaneLayout {
    /// # Safety
    /// `data` must be readable for `len` initialized bytes in one allocation,
    /// immutable and live for `'a`. Preflight proves layout, not ownership.
    unsafe fn view<'a>(self) -> RawPlane<'a> {
        // SAFETY: the caller guarantees the allocation and lifetime; preflight
        // checked non-null, positive geometry, stride and isize-bounded span.
        let data = unsafe { std::slice::from_raw_parts(self.data, self.len) };
        RawPlane {
            data,
            stride: self.stride,
            width: self.width,
            height: self.height,
            bytes: self.bytes,
        }
    }
}

/// Pure header preflight: no pointer is dereferenced, even if a later plane
/// fails. Keep this separate from `view` so validation cannot borrow early.
fn preflight_planes(
    frame: &FFMS_Frame,
    details: &VideoDetails,
    index: usize,
) -> Result<(PlaneLayout, Option<(PlaneLayout, PlaneLayout)>)> {
    validate_metadata(frame, details, index)?;
    let bytes = sample_bytes(details.bit_depth)?;
    let (width, height) = (details.width, details.height);
    let luma = plane(frame, 0, width, height, bytes, index)?;
    let Some((sub_x, sub_y)) = details.chroma_sampling.subsample_ratio() else {
        return Ok((luma, None));
    };
    // Raw FFMS2 planes include partial chroma blocks at odd luma edges;
    // FrameBuilder's tile divisibility restrictions do not apply here.
    let (chroma_width, chroma_height) = (
        width.div_ceil(NonZeroUsize::from(sub_x).get()),
        height.div_ceil(NonZeroUsize::from(sub_y).get()),
    );
    let cb = plane(frame, 1, chroma_width, chroma_height, bytes, index)?;
    let cr = plane(frame, 2, chroma_width, chroma_height, bytes, index)?;
    Ok((luma, Some((cb, cr))))
}

/// Checks one plane header without reading its pointer.
fn plane(
    frame: &FFMS_Frame,
    plane: usize,
    width: usize,
    height: usize,
    bytes: usize,
    index: usize,
) -> Result<PlaneLayout> {
    ensure!(
        !frame.Data[plane].is_null(),
        "FFMS2 frame {index} plane {plane}: null data pointer"
    );
    let stride = usize::try_from(frame.Linesize[plane])
        .ok()
        .filter(|stride| *stride > 0)
        .ok_or_else(|| anyhow::anyhow!("FFMS2 frame {index} plane {plane}: unusable stride"))?;
    let len = plane_span(stride, width, height, bytes)
        .ok_or_else(|| anyhow::anyhow!("FFMS2 frame {index} plane {plane}: invalid or overflowing span (stride {stride}, {width}x{height}, {bytes} bytes/sample)"))?;
    Ok(PlaneLayout {
        data: frame.Data[plane],
        stride,
        width,
        height,
        bytes,
        len,
    })
}

fn plane_span(stride: usize, width: usize, height: usize, bytes: usize) -> Option<usize> {
    if width == 0 || height == 0 || !matches!(bytes, 1 | 2) {
        return None;
    }
    let row = width.checked_mul(bytes)?;
    if stride < row {
        return None;
    }
    // Last-row padding need not be allocated; borrow only through its pixels.
    let span = (height - 1).checked_mul(stride)?.checked_add(row)?;
    isize::try_from(span).ok().map(|_| span)
}

/// Bytes per sample for a bit depth this reader can address.
fn sample_bytes(bit_depth: usize) -> Result<usize> {
    match bit_depth {
        8 => Ok(1),
        9..=16 => Ok(2),
        depth => bail!("{depth}-bit samples cannot be read"),
    }
}

/// The message FFMS2 wrote into `error`, if any.
fn error_message(error: &FFMS_ErrorInfo) -> String {
    if error.Buffer.is_null() || error.BufferSize <= 0 {
        return "no error detail".to_owned();
    }
    // SAFETY: FFMS2 null-terminates what it writes inside `Buffer`, which the
    // caller owns for `BufferSize` bytes.
    unsafe { std::ffi::CStr::from_ptr(error.Buffer) }.to_string_lossy().into_owned()
}

/// Whether requests issued for `[first, requested)` are still outstanding.
/// Arrivals count from zero while indices are absolute, so a resumed run with
/// `first > 0` must not compare arrivals against the absolute index — that
/// would never settle and the window would never close.
fn outstanding(arrived: usize, first: usize, requested: usize) -> bool {
    arrived < requested.saturating_sub(first)
}

/// VapourSynth: frames are requested ahead of demand and handed out as they
/// arrive.
struct VsFeed<'core> {
    node: Node<'core>,
    /// Absolute index of the first frame this feed reads.
    first: usize,
    end: usize,
    next: usize,
    requested: usize,
    window: usize,
    /// Frames received per index, plus how many callbacks have run, so `drop`
    /// can wait out requests still in flight.
    state: Received<'core>,
    /// The frame the current [`RawFrame`] views; the next call replaces it,
    /// which is what retires the previous frame's planes.
    current: Option<FrameRef<'core>>,
}

/// The window state shared with VapourSynth's frame callbacks.
type Received<'core> = Arc<(
    Mutex<(BTreeMap<usize, Result<FrameRef<'core>, String>>, usize)>,
    Condvar,
)>;

impl<'core> VsFeed<'core> {
    fn new(node: Node<'core>, first: usize, end: usize) -> Self {
        Self {
            node,
            first,
            end,
            next: first,
            requested: first,
            window: request_window().max(1),
            state: Arc::new((Mutex::new((BTreeMap::new(), 0)), Condvar::new())),
            current: None,
        }
    }

    /// Runs up to `window` requests ahead of the frame being handed out, so
    /// decode and consumption overlap without the whole clip staying resident.
    fn request_ahead(&mut self) {
        while self.requested < self.end && self.requested - self.next < self.window {
            let state = Arc::clone(&self.state);
            let position = self.requested;
            self.node.get_frame_async(position, move |frame, _index, _node| {
                let (lock, condvar) = &*state;
                let mut pending = lock.lock().expect("mutex should acquire lock");
                pending.0.insert(position, frame.map_err(|error| error.to_string()));
                pending.1 += 1;
                condvar.notify_all();
            });
            self.requested += 1;
        }
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame<'_>>> {
        if self.next >= self.end {
            return Ok(None);
        }
        let position = self.next;
        self.request_ahead();

        let frame = {
            let (lock, condvar) = &*self.state;
            let pending = lock.lock().expect("mutex should acquire lock");
            let mut pending = condvar
                .wait_while(pending, |state| !state.0.contains_key(&position))
                .expect("Condvar should be notified");
            pending.0.remove(&position).expect("requested frame arrives")
        };
        self.next = position + 1;

        let frame = frame.map_err(|error| anyhow::anyhow!("get frame {position}: {error}"))?;
        // Keeping the frame here is what keeps the returned planes valid until
        // the next call replaces it.
        self.current = Some(frame);
        let frame = self.current.as_ref().expect("frame just stored");
        let luma = vs_plane(frame, 0)?;
        let chroma = (frame.format().plane_count() >= 3)
            .then(|| -> Result<_> { Ok((vs_plane(frame, 1)?, vs_plane(frame, 2)?)) })
            .transpose()?;
        Ok(Some(RawFrame {
            luma,
            chroma,
            bit_depth: usize::from(frame.format().bits_per_sample()),
        }))
    }
}

impl Drop for VsFeed<'_> {
    fn drop(&mut self) {
        // A callback still running would write into state this drop frees.
        let (lock, condvar) = &*self.state;
        let (first, requested) = (self.first, self.requested);
        drop(
            condvar
                .wait_while(lock.lock().expect("mutex should acquire lock"), |state| {
                    outstanding(state.1, first, requested)
                })
                .expect("Condvar should be notified"),
        );
    }
}

/// Addresses one VapourSynth plane in place, without copying it.
fn vs_plane<'a>(frame: &FrameRef<'a>, plane: usize) -> Result<RawPlane<'a>> {
    let format = frame.format();
    let height = frame.height(plane);
    let stride = frame.stride(plane);
    let len = stride
        .checked_mul(height)
        .ok_or_else(|| anyhow::anyhow!("VapourSynth plane size overflows"))?;
    // SAFETY: VapourSynth guarantees `stride * height` readable bytes at
    // `data_ptr` for as long as the frame is alive.
    let data = unsafe { std::slice::from_raw_parts(frame.data_ptr(plane), len) };
    RawPlane::new(
        data,
        stride,
        frame.width(plane),
        height,
        usize::from(format.bytes_per_sample()),
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "{}-bit VapourSynth samples cannot be read",
            format.bits_per_sample()
        )
    })
}

#[cfg(test)]
mod tests {
    use av_decoders::v_frame::chroma::ChromaSubsampling;

    use super::*;

    fn frame_details(width: usize, height: usize, bit_depth: usize) -> VideoDetails {
        VideoDetails {
            width,
            height,
            bit_depth,
            chroma_sampling: ChromaSubsampling::Yuv420,
            frame_rate: av_decoders::Rational32::new(24, 1),
            total_frames: Some(1),
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    fn header(data: &[u8], width: i32, height: i32, format: &str) -> FFMS_Frame {
        // SAFETY: this C header contains only numeric fields and raw pointers;
        // zero is valid for each field. Plane allocations remain caller-owned.
        let mut frame: FFMS_Frame = unsafe { std::mem::zeroed() };
        let format = std::ffi::CString::new(format).expect("pixel format name");
        // SAFETY: the format name is a live C string; lookup reads no planes.
        frame.ConvertedPixelFormat = unsafe { ffms2_sys::FFMS_GetPixFmt(format.as_ptr()) };
        frame.ScaledWidth = width;
        frame.ScaledHeight = height;
        frame.EncodedWidth = width;
        frame.EncodedHeight = height;
        frame.Data = [data.as_ptr(); 4];
        frame.Linesize = [32; 4];
        frame
    }

    #[test]
    fn converted_metadata_mismatch_rejected() {
        let details = frame_details(16, 1080, 8);
        // Overallocate every plane for the OLD configured-height read as well
        // as every claimed format. RED must never rely on invalid memory.
        let data = vec![0; 32 * 1080];
        for (width, height, format) in [
            (16, 800, "yuv420p"),
            (15, 1080, "yuv420p"),
            (16, 1080, "yuv444p"),
            (16, 1080, "yuv420p10le"),
            (16, 1080, "yuv420p10be"),
        ] {
            let frame = header(&data, width, height, format);
            // SAFETY: all planes have 1080 fully allocated 32-byte rows, live
            // through the call, including when baseline ignores the metadata.
            let result = unsafe { planes_of(&frame, &details, 42) };
            assert!(result.is_err(), "accepted {width}x{height} {format}");
            let message = result.err().expect("metadata error").to_string();
            assert!(message.contains("frame 42"), "{message}");
            assert!(message.contains("16x1080"), "{message}");
            assert!(message.contains(&format!("{width}x{height}")), "{message}");
        }
    }

    #[test]
    fn metadata_precedes_all_plane_headers() {
        let details = frame_details(16, 1080, 10);
        let mut frame = header(&[], 16, 800, "yuv420p10le");
        frame.Data = [std::ptr::null(); 4];
        for (width, height, format) in [
            (16, 800, "yuv420p10le"),
            (0, 1080, "yuv420p10le"),
            (-2, 1080, "yuv420p10le"),
            (16, 0, "yuv420p10le"),
            (16, -2, "yuv420p10le"),
            (-1, 1080, "yuv420p10le"),
            (16, 1080, "yuv444p10le"),
            (16, 1080, "yuv420p12le"),
            (16, 1080, "yuv420p10be"),
            (16, 1080, "no-such-format"),
        ] {
            frame.ScaledWidth = width;
            frame.ScaledHeight = height;
            let name = std::ffi::CString::new(format).expect("format name");
            // SAFETY: lookup only, no plane read.
            frame.ConvertedPixelFormat = unsafe { ffms2_sys::FFMS_GetPixFmt(name.as_ptr()) };
            let error = preflight_planes(&frame, &details, 7)
                .err()
                .expect("reject metadata")
                .to_string();
            assert!(error.contains("expected 16x1080"), "{error}");
            assert!(error.contains("frame 7"), "{error}");
            assert!(error.contains("format"), "{error}");
            assert!(
                !error.contains("null data"),
                "metadata must precede planes: {error}"
            );
        }
    }

    #[test]
    fn all_planes_preflight_without_views() {
        let details = frame_details(16, 8, 8);
        // These deliberately have NO readable plane allocation. Only the
        // production pure preflight is called, never `planes_of`/`view`.
        let mut frame = header(&[], 16, 8, "yuv420p");
        frame.Data = [std::ptr::dangling(); 4];
        for plane in 0..3 {
            frame.Data[plane] = std::ptr::null();
            let error =
                preflight_planes(&frame, &details, 9).err().expect("null plane").to_string();
            assert!(
                error.contains(&format!("frame 9 plane {plane}: null")),
                "{error}"
            );
            frame.Data[plane] = std::ptr::dangling();
            for stride in [-1, 0, if plane == 0 { 15 } else { 7 }] {
                frame.Linesize[plane] = stride;
                assert!(preflight_planes(&frame, &details, 9).is_err());
            }
            frame.Linesize[plane] = 32;
        }
        // In particular an invalid V is discovered with unreadable Y and U:
        // the safe function has no view callback or unsafe operation at all.
        assert!(preflight_planes(&frame, &details, 9).is_ok());
    }

    #[test]
    fn spans_checked_and_last_row_padding_not_required() {
        for (stride, width, height, bytes) in [
            (1, 0, 1, 1),
            (1, 1, 0, 1),
            (1, 1, 1, 0),
            (1, 1, 1, 4),
            (1, 2, 1, 1),
            (usize::MAX, usize::MAX, 1, 2),
            (usize::MAX, 1, 3, 1),
            (isize::MAX as usize, 1, 2, 1),
            (usize::MAX, 1, 2, 1),
        ] {
            assert_eq!(plane_span(stride, width, height, bytes), None);
        }
        assert_eq!(plane_span(isize::MAX as usize, 1, 1, 1), Some(1));
        let mut details = frame_details(2, 2, 10);
        details.chroma_sampling = ChromaSubsampling::Monochrome;
        let data = [0x23, 0x01, 0x45, 0x02, 99, 99, 0x67, 0x03, 0x89, 0x01];
        let mut frame = header(&data, 2, 2, "gray10le");
        frame.Data[1..].fill(std::ptr::null());
        frame.Linesize = [6, -1, -1, -1];
        // SAFETY: exactly (2-1)*6+4 = 10 bytes, with no final-row padding.
        let raw = unsafe { planes_of(&frame, &details, 0) }.expect("minimal allocation");
        assert_eq!(raw.luma.data.len(), 10);
        assert_eq!(raw.luma.sample(1, 0), 0x0367);
        assert_eq!(raw.luma.sample(1, 1), 0x0189);
        assert!(raw.chroma.is_none());
    }

    #[test]
    fn valid_layouts_and_documented_unscaled_sentinel() {
        let data = vec![0x01; 32 * 8];
        for (depth, format) in [(8, "yuv420p"), (10, "yuv420p10le"), (12, "yuv420p12le")] {
            let details = frame_details(5, 3, depth);
            let mut frame = header(&data, 5, 3, format);
            // Encoded geometry is irrelevant when actual converted size is set.
            frame.EncodedWidth = 100;
            frame.EncodedHeight = 800;
            // SAFETY: all three 32*8 allocations cover the visible spans.
            let raw = unsafe { planes_of(&frame, &details, 0) }.expect("valid output");
            let (u, v) = raw.chroma.expect("420 chroma");
            assert_eq!((u.width(), u.height(), v.width(), v.height()), (3, 2, 3, 2));
            frame.ScaledWidth = -1;
            frame.ScaledHeight = -1;
            assert!(preflight_planes(&frame, &details, 1).is_err());
            frame.EncodedWidth = 5;
            frame.EncodedHeight = 3;
            assert!(preflight_planes(&frame, &details, 1).is_ok());
        }
        for depth in [9, 16] {
            // av-decoders 0.12 does not accept these as native decoder output.
            assert!(output_format_id(&frame_details(5, 3, depth)).is_err());
        }
        let details = frame_details(0, 3, 8);
        let frame = header(&data, 0, 3, "yuv420p");
        assert!(preflight_planes(&frame, &details, 1).is_err());
        let frame = header(&data, 5, 3, "yuv420p");
        let mut details = frame_details(5, 3, 8);
        assert!(validate_metadata(&frame, &details, 1).is_ok());
        // On 64-bit, unchecked i32/u32 narrowing aliases this positive width
        // back to 5. Use metadata only: never attempt an oversized plane view.
        details.width = usize::try_from((1_u64 << 32) + 5).unwrap_or(i32::MAX as usize + 1);
        if usize::BITS > 32 {
            assert_eq!(details.width as i32, 5);
        }
        assert!(validate_metadata(&frame, &details, 1).is_err());
    }

    #[test]
    fn only_supported_sample_widths_read() {
        assert_eq!(sample_bytes(8).expect("8-bit"), 1);
        assert_eq!(sample_bytes(10).expect("10-bit"), 2);
        assert_eq!(sample_bytes(12).expect("12-bit"), 2);
        assert_eq!(sample_bytes(16).expect("16-bit"), 2);
        assert!(sample_bytes(0).is_err());
        assert!(sample_bytes(32).is_err(), "32-bit float must be rejected");
    }

    #[test]
    fn error_detail_is_reported_verbatim() {
        let mut buffer: Vec<c_char> =
            b"failed to decode\0".iter().map(|&byte| byte as c_char).collect();
        let error = FFMS_ErrorInfo {
            ErrorType: 1,
            SubType: 0,
            BufferSize: i32::try_from(buffer.len()).expect("buffer fits in i32"),
            Buffer: buffer.as_mut_ptr(),
        };
        assert_eq!(error_message(&error), "failed to decode");
    }

    /// An absent error buffer must not be dereferenced.
    #[test]
    fn missing_error_detail_is_reported_as_such() {
        let error = FFMS_ErrorInfo {
            ErrorType: 1,
            SubType: 0,
            BufferSize: 0,
            Buffer: std::ptr::null_mut(),
        };
        assert_eq!(error_message(&error), "no error detail");
    }

    /// Regression: arrivals are counted from zero while `requested` is an
    /// absolute frame index. Comparing them directly never settles once a
    /// resumed run starts past frame 0, hanging the detector forever.
    #[test]
    fn a_resumed_window_closes_once_its_requests_arrive() {
        // Resumed at frame 240, 480 frames read, all delivered.
        assert!(
            !outstanding(480, 240, 720),
            "every issued request arrived, so the window must close"
        );
        // The last delivery is still pending.
        assert!(outstanding(479, 240, 720));
        // A run from the start behaves the same way.
        assert!(!outstanding(0, 0, 0));
        assert!(outstanding(0, 0, 8));
        assert!(!outstanding(8, 0, 8));
    }

    /// A backend that does not hand over integer samples in the widths this
    /// reader addresses must be rejected rather than misinterpreted.
    #[test]
    fn raw_plane_rejects_unsupported_layouts() {
        let data = vec![0u8; 64 * 16];
        assert!(RawPlane::new(&data, 64, 16, 16, 1).is_some());
        assert!(RawPlane::new(&data, 64, 16, 16, 2).is_some());
        // 32-bit float (VapourSynth) and padded 4-byte layouts are not samples.
        assert!(RawPlane::new(&data, 64, 16, 16, 4).is_none());
        assert!(RawPlane::new(&data, 64, 16, 16, 0).is_none());
        // A buffer too small for the last row's first sample.
        assert!(RawPlane::new(&data[..15], 16, 16, 16, 1).is_none());
        // A stride narrower than a row would make successive rows overlap.
        assert!(RawPlane::new(&data, 8, 16, 16, 1).is_none());
        assert!(RawPlane::new(&data, 16, 16, 0, 1).is_none());
    }

    /// Rows address only visible samples: stride padding stays unread, and
    /// samples decode little-endian at either width.
    #[test]
    fn rows_and_samples_read_visible_pixels_only() {
        // 4x2 8-bit samples with 2 padding bytes at the end of each 6-byte row.
        let data = vec![0, 1, 2, 3, 91, 92, 4, 5, 6, 7, 93, 94];
        let plane = RawPlane::new(&data, 6, 4, 2, 1).expect("valid 8-bit plane");
        assert_eq!(plane.width(), 4);
        assert_eq!(plane.height(), 2);
        assert_eq!(plane.bytes_per_sample(), 1);
        assert_eq!(plane.row(0), &[0, 1, 2, 3][..]);
        assert_eq!(plane.row(1), &[4, 5, 6, 7][..]);
        assert_eq!(plane.sample(0, 0), 0);
        assert_eq!(plane.sample(1, 3), 7);

        // 16-bit samples read little-endian, not native-endian.
        let wide = vec![0xff, 0xff, 2, 0, 91, 92];
        let plane = RawPlane::new(&wide, 6, 2, 1, 2).expect("valid 16-bit plane");
        assert_eq!(plane.row(0), &[0xff, 0xff, 2, 0][..]);
        assert_eq!(plane.sample(0, 0), u32::from(u16::MAX));
        assert_eq!(plane.sample(0, 1), 2);
    }
}
