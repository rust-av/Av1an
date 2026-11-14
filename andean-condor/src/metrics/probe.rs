//! The decode/probe driver shared by every native metric.
//!
//! Comparing an encode against its source means pairing two clips frame by
//! frame and handing each pair to a metric. Everything that is the same for
//! every metric lives here: choosing a reader per side, seeking or advancing to
//! each selected frame, checking the two clips agree on geometry, and calling
//! back with the planes of one pair.
//!
//! The plane descriptor is declared locally rather than imported from a metric
//! crate. Keeping [`PlaneSource`] here is what lets this module stay
//! metric-agnostic: a metric that needs a different type adapts the set inside
//! its own `submit` callback instead of the driver depending on that crate.

use std::{
    marker::PhantomData,
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context as _, Result, bail};
use itertools::Itertools;
use vapoursynth::frame::FrameRef;

/// Returned when a pass is abandoned because the user cancelled it.
///
/// A unit struct rather than a formatted error, since the enclosing sequence
/// already knows what cancellation means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the quality check was cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// One decoded plane, described without copying it.
///
/// The fields mirror what a metric library needs to read a plane: dimensions in
/// samples, the row spacing, the offset of the first visible sample, and the
/// pointer to the allocation itself.
///
/// A `PlaneSource` aliases the frame it came from and so is only valid while
/// that frame is alive. Every producer here hands the set to a callback that
/// consumes it before the frame is dropped.
pub struct PlaneSource<'a> {
    /// Width in pixels (samples per row, not bytes).
    width: usize,
    /// Height in pixels.
    height: usize,
    /// Bytes from one row to the next. May be negative for bottom-up images.
    stride: isize,
    /// Byte offset of the first visible sample from `data`.
    data_origin: usize,
    /// Pointer to the start of the underlying allocation.
    data: *const u8,
    /// Bytes per sample.
    bytes_per_sample: usize,
    readable_len: usize,
    _frame: PhantomData<&'a [u8]>,
}

impl PlaneSource<'_> {
    /// Visible width in samples.
    #[inline]
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }
    /// Visible height in rows.
    #[inline]
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }
    /// Row spacing in bytes.
    #[inline]
    #[must_use]
    pub const fn stride(&self) -> isize {
        self.stride
    }
    /// First visible sample's byte offset.
    #[inline]
    #[must_use]
    pub const fn data_origin(&self) -> usize {
        self.data_origin
    }
    /// Native sample storage size in bytes.
    #[inline]
    #[must_use]
    pub const fn bytes_per_sample(&self) -> usize {
        self.bytes_per_sample
    }
    /// Backing address; only visible sample bytes are guaranteed initialized.
    #[inline]
    #[must_use]
    pub const fn data_ptr(&self) -> *const u8 {
        self.data
    }
    /// Known backing span in bytes, including row gaps but not guessed final padding.
    #[inline]
    #[must_use]
    pub const fn readable_len(&self) -> usize {
        self.readable_len
    }
}

/// The three planes of a planar YUV frame, in Y, U, V order.
pub type PlaneSet<'a> = [PlaneSource<'a>; 3];

/// Which index space the encoded output is addressed in.
///
/// The call sites pass encodes with different timelines, so conflating them
/// pairs the wrong frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputIndexing {
    /// The output holds only the selected frames, compacted.
    ///
    /// This is a *probe* encode: `ParallelEncoder` trims each task's input to
    /// `frame_indices`, so the concat contains exactly the probed frames,
    /// re-indexed from zero. Output frame *k* encodes source frame
    /// `selected[k]`.
    ///
    /// This matches what the VapourSynth path does for target-quality, which
    /// trims the reference to the selected indices and `Splice`s it while
    /// leaving the distorted node as a bare `Source` — both sides end up
    /// compacted identically.
    Compacted,

    /// The output covers the whole clip and is addressed by source frame
    /// number.
    ///
    /// This is a completed encode being re-scored, so a selected frame sits at
    /// the same index in both clips. It matches what the VapourSynth path does
    /// for quality-check, which trims *both* nodes by the same global indices.
    Aligned,
}

/// The encoded-output frame that pairs with the source frame at `position`
/// within the selection.
///
/// The reference is always the full source clip, so its index is
/// `selected[position]`. The distorted output is addressed per `indexing`, and
/// conflating the two spaces pairs unrelated frames — or, for a compacted
/// output, runs past the end of the clip entirely.
#[must_use]
#[inline]
pub fn output_index(position: usize, source_index: usize, indexing: OutputIndexing) -> usize {
    match indexing {
        OutputIndexing::Compacted => position,
        OutputIndexing::Aligned => source_index,
    }
}

/// Validate scene partitions without sorting or hiding source discontinuities.
/// Empty scenes are allowed, but cannot create scoring sessions.
pub(crate) fn validate_scene_selection(scenes: &[&[usize]]) -> Result<usize> {
    let mut total = 0usize;
    let mut previous = None;
    for scene in scenes {
        total = total.checked_add(scene.len()).context("selected frame count overflow")?;
        for &index in *scene {
            if index == usize::MAX || previous.is_some_and(|previous| previous >= index) {
                bail!("scene source indices must be strictly increasing and below usize::MAX");
            }
            previous = Some(index);
        }
    }
    Ok(total)
}

/// One side of the comparison, read by the cheapest method available for it.
///
/// Each side picks its own reader, independently. In both call sites the
/// reference is a VapourSynth script while the distorted side is an encoded
/// file opened through FFMS2, so a single "both must be VapourSynth" fast path
/// would never be taken.
pub enum Side<'decoder> {
    /// Read straight off a VapourSynth node.
    ///
    /// `av-decoders` would copy every plane into a `v_frame::Frame` first,
    /// which for 1080p is ~5.9 MB per frame per side. The node's planes are
    /// already strided buffers, so pointing at them skips that copy
    /// entirely.
    ///
    /// The node is built once and held for the whole run. `get_output` keeps
    /// the decoder borrowed for the node's lifetime, which is fine because
    /// a VapourSynth node seeks per frame inside [`Side::with_planes`], so
    /// the decoder itself is never touched again. Holding one also avoids
    /// re-running any registered `ModifyNode` on every frame.
    Node(vapoursynth::node::Node<'decoder>),

    /// Read through `av-decoders`: the only option for FFMS2, FFmpeg and y4m.
    ///
    /// `next` is the frame the following read will return: `advance` sets it
    /// once the decoder is positioned, and `with_planes` moves it past the
    /// frame it just read. Backends that cannot seek rely on it to move forward
    /// by decoding and discarding the frames in between.
    Native {
        decoder: &'decoder mut av_decoders::Decoder,
        bit_depth: usize,
        next: usize,
        seekable: bool,
    },
}

impl<'decoder> Side<'decoder> {
    /// Open `decoder` as a scoring side.
    ///
    /// # Errors
    ///
    /// Returns an error when the decoder is VapourSynth but its node cannot be
    /// built. Building it here proves the graph works, so a broken script is
    /// reported before any frame is decoded.
    #[inline]
    pub fn open(decoder: &'decoder mut av_decoders::Decoder, bit_depth: usize) -> Result<Self> {
        // Probed first, in a borrow that ends at the end of the call. Asking
        // `open` for the node directly would not work: `Node<'decoder>` in the
        // return type forces the decoder's borrow to outlive the function,
        // which overlaps the `&mut` the native path needs.
        if is_vapoursynth(decoder) {
            let impl_ = decoder
                .get_vapoursynth_impl()
                .expect("the decoder was just probed as VapourSynth");
            let node = impl_.get_output(impl_.get_output_index(), impl_.get_node_modifier())?;
            return Ok(Self::Node(node));
        }

        // Seeking to frame 0 is a no-op for a backend that supports seeking,
        // and an error for one that does not.
        let seekable = decoder
            .seek_to_frame(0)
            .or_else(|error| {
                // `EndOfFile` for an empty clip still means seeking is
                // implemented.
                matches!(error, av_decoders::DecoderError::EndOfFile).then_some(()).ok_or(error)
            })
            .is_ok();

        Ok(Self::Native {
            seekable,
            decoder,
            bit_depth,
            next: 0,
        })
    }

    /// Whether this side can reach an arbitrary frame without decoding the ones
    /// in between.
    #[inline]
    #[must_use]
    pub fn is_seekable(&self) -> bool {
        !matches!(
            self,
            Self::Native {
                seekable: false,
                ..
            }
        )
    }

    /// Point at the planes of frame `index` and hand them to `submit`.
    ///
    /// The frame cannot outlive this call -- a `v_frame::Frame` is only valid
    /// until the decoder's next read, and a VapourSynth `FrameRef` borrows the
    /// node -- so the planes go straight to a callback instead of being
    /// returned. `submit` is what copies them into the metric.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be decoded or has no scorable
    /// plane layout.
    ///
    /// A frame-owned callback cannot return a borrowed descriptor:
    /// ```compile_fail
    /// use andean_condor::metrics::probe::Side;
    /// fn escape(side: &mut Side<'_>) {
    ///     let planes = side.with_planes(0, |planes| Ok(planes)).unwrap();
    ///     println!("{}", planes[0].width());
    /// }
    /// ```
    #[inline]
    pub fn with_planes<T>(
        &mut self,
        index: usize,
        submit: impl for<'frame> FnOnce(PlaneSet<'frame>) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Node(node) => {
                let frame = node
                    .get_frame(index)
                    .map_err(|error| anyhow::anyhow!("failed to read frame {index}: {error}"))?;
                submit(vapoursynth_planes(&frame)?)
            },
            Self::Native {
                decoder,
                bit_depth,
                next,
                ..
            } => {
                let read = with_native_planes(decoder, *bit_depth, index, submit);
                // A successful read consumed frame `index`, so the tracked
                // position moves past it. A failure leaves the position
                // untouched; the pass stops either way.
                if read.is_ok() {
                    *next = index + 1;
                }
                read
            },
        }
    }

    /// Move to `target`, so the next read returns it.
    ///
    /// A VapourSynth node is random access, so this is a no-op for it.
    ///
    /// # Errors
    ///
    /// Returns an error if `target` cannot be reached, either because seeking
    /// failed or because a backend that cannot seek was asked to go backwards.
    #[inline]
    pub fn advance(&mut self, target: usize) -> Result<()> {
        let Self::Native {
            decoder,
            bit_depth,
            next,
            seekable,
        } = self
        else {
            return Ok(());
        };

        if *next == target {
            return Ok(());
        }

        if *seekable {
            decoder
                .seek_to_frame(target)
                .map_err(|error| anyhow::anyhow!("failed to seek to frame {target}: {error}"))?;
        } else if target > *next {
            // Decode and discard the frames in between. Skipped frames are
            // still decoded, just never scored, so the planes go to
            // a no-op.
            for skipped in *next..target {
                with_native_planes(decoder, *bit_depth, skipped, |_| Ok(()))?;
            }
        } else {
            bail!("cannot go back to frame {target} from {next} with a decoder that cannot seek");
        }

        // The seek or the discard run has positioned the decoder at `target`,
        // which is what the following read returns; `with_planes` moves past it
        // once the frame is read.
        *next = target;
        Ok(())
    }
}

/// Whether this decoder reads through a VapourSynth node.
#[inline]
fn is_vapoursynth(decoder: &mut av_decoders::Decoder) -> bool {
    decoder.get_vapoursynth_impl().is_some()
}

/// Read clip metadata, including the first output frame's color properties for
/// VapourSynth. Its ordinary details API only reports geometry and frame rate.
/// FFMS2/FFmpeg already populate color fields; Y4M leaves them unknown.
///
/// # Errors
///
/// Returns an error if the VapourSynth output or its first frame cannot be read.
#[inline]
pub(super) fn video_details(
    decoder: &mut av_decoders::Decoder,
) -> Result<av_decoders::VideoDetails> {
    if let Some(decoder) = decoder.get_vapoursynth_impl() {
        Ok(decoder.get_extended_video_details()?)
    } else {
        Ok(*decoder.get_video_details())
    }
}

/// Decode one frame through `av-decoders` and hand its planes to `submit`.
///
/// The pixel type has to be chosen before the read, so the dispatch on bit
/// depth happens here rather than at every call site.
///
/// This is callback-shaped rather than returning the [`PlaneSet`] because the
/// set aliases the decoded frame, which is dropped when this returns. Handing
/// the planes to `submit` inside the call is what keeps that from being a
/// use-after-free.
///
/// # Errors
///
/// Returns an error if the frame cannot be decoded, or whatever `submit`
/// returns.
#[inline]
fn with_native_planes<T>(
    decoder: &mut av_decoders::Decoder,
    bit_depth: usize,
    index: usize,
    submit: impl for<'frame> FnOnce(PlaneSet<'frame>) -> Result<T>,
) -> Result<T> {
    macro_rules! read {
        ($pixel:ty) => {{
            let frame = decoder
                .read_video_frame::<$pixel>()
                .map_err(|error| anyhow::anyhow!("failed to decode frame {index}: {error}"))?;
            submit(native_planes(&frame)?)
        }};
    }

    // `av-decoders` requires the pixel type to match the video's bit depth.
    if bit_depth > 8 { read!(u16) } else { read!(u8) }
}

/// Describe a `v_frame` frame as its three scorable planes.
///
/// The returned sources alias `frame`, so `frame` must outlive them;
/// [`with_native_planes`] enforces that by never returning the set.
///
/// # Errors
///
/// Returns an error when the frame has no chroma planes, which is the case for
/// monochrome content no planar metric can score.
#[inline]
fn native_planes<T>(frame: &av_decoders::v_frame::frame::Frame<T>) -> Result<PlaneSet<'_>>
where
    T: av_decoders::v_frame::pixel::Pixel,
{
    let bytes_per_sample = size_of::<T>();
    let (Some(u_plane), Some(v_plane)) = (&frame.u_plane, &frame.v_plane) else {
        bail!("frame has no chroma planes; a metric requires 4:2:0, 4:2:2 or 4:4:4 content");
    };

    let [y, u, v] = [&frame.y_plane, u_plane, v_plane].map(|plane| {
        let geometry = plane.geometry();
        let stride = geometry
            .stride()
            .checked_mul(bytes_per_sample)
            .and_then(|n| isize::try_from(n).ok())
            .context("native plane stride overflow")?;
        let origin = geometry
            .data_origin()
            .checked_mul(bytes_per_sample)
            .context("native plane origin overflow")?;
        // v_frame 0.7's Pixel is sealed to padding-free u8/u16. Plane::data
        // supplies actual initialized storage and the return lifetime borrows frame.
        Ok::<_, anyhow::Error>(PlaneSource {
            width: geometry.width(),
            height: geometry.height(),
            stride,
            data_origin: origin,
            data: plane.data().as_ptr().cast::<u8>(),
            bytes_per_sample,
            readable_len: std::mem::size_of_val(plane.data()),
            _frame: PhantomData,
        })
    });
    Ok([y?, u?, v?])
}

/// Describe a VapourSynth frame as its three scorable planes.
///
/// The returned sources point into `frame`'s plane allocations, so `frame` must
/// outlive their use. Every caller here passes them straight to `submit`, which
/// copies them before the frame is dropped.
///
/// # Errors
///
/// Returns an error for a layout the metrics here cannot score. They are
/// defined on planar YUV, so RGB or a non-planar layout is rejected rather
/// than reinterpreted, which would silently compare the wrong channels.
#[inline]
fn vapoursynth_planes<'a>(frame: &'a FrameRef<'_>) -> Result<PlaneSet<'a>> {
    use vapoursynth::format::ColorFamily;

    let format = frame.format();
    if format.color_family() != ColorFamily::YUV || format.plane_count() != 3 {
        bail!(
            "a metric requires planar YUV, but the frame is in the {:?} family",
            format.color_family()
        );
    }

    let bytes_per_sample = usize::from(format.bytes_per_sample());

    let [y, u, v] = [0, 1, 2].map(|index| {
        let height = frame.height(index);
        let stride = frame.stride(index);
        let last_row = height.checked_sub(1).context("empty VapourSynth plane")?;
        // data_row guarantees the last visible row, NOT trailing stride padding.
        // Row gaps can be uninitialized, so never manufacture &[u8] over them.
        let row_bytes = frame
            .width(index)
            .checked_mul(bytes_per_sample)
            .context("VapourSynth row overflow")?;
        let readable_len = last_row
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(row_bytes))
            .filter(|&len| isize::try_from(len).is_ok())
            .context("VapourSynth span overflow")?;
        Ok::<_, anyhow::Error>(PlaneSource {
            width: frame.width(index),
            height: frame.height(index),
            stride: isize::try_from(stride).context("VapourSynth stride overflow")?,
            data_origin: 0,
            data: frame.data_ptr(index),
            bytes_per_sample,
            readable_len,
            _frame: PhantomData,
        })
    });
    Ok([y?, u?, v?])
}

/// Read every frame in `selected` from both sides and hand each pair to
/// `submit`, returning one value per selected frame.
///
/// This owns the whole loop so that every metric seeks, validates and reads
/// identically; a metric only supplies `submit`, which receives the position in
/// the selection plus both plane sets.
///
/// `validate` runs once, after both sides are opened and before any frame is
/// decoded, so a metric can reject the clip pair before spending any work on
/// it. [`check_geometry`] is the shared implementation.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See [`output_index`].
///
/// # Cost
///
/// When a side can seek, each selected frame is reached directly, so frames
/// outside the selection are never decoded at all — the same saving the plugin
/// metrics get by trimming frames out of the graph. Backends that cannot seek
/// read forward and discard instead, which avoids *scoring* skipped frames but
/// still decodes them.
///
/// `selected` need not be sorted; frames are visited in the order given.
///
/// `cancelled` is polled once per frame, so a long pass stops promptly when the
/// user aborts instead of running to completion.
///
/// # Errors
///
/// Returns an error when `validate` rejects the two clips, when a selected
/// frame cannot be reached or decoded, when `cancelled` is set, or with
/// whatever `submit` returns.
#[inline]
pub fn probe_selected_frames<T, R>(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
    validate: impl FnOnce(av_decoders::VideoDetails, av_decoders::VideoDetails) -> Result<()>,
    submit: impl for<'r, 'd> FnMut(usize, PlaneSet<'r>, PlaneSet<'d>) -> Result<R>,
) -> Result<Vec<R>> {
    probe_selected_frames_from::<R>(
        reference, distorted, selected, indexing, 0, 0, cancelled, validate, submit,
    )
}

/// The same decode loop for a scene starting inside a compacted probe output.
/// Validate the entire output range before metadata, opening sides or reads.
#[allow(clippy::too_many_arguments)]
pub(super) fn probe_selected_frames_from<R>(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    selected: &[usize],
    indexing: OutputIndexing,
    output_start: usize,
    reference_next: usize,
    cancelled: Option<&AtomicBool>,
    validate: impl FnOnce(av_decoders::VideoDetails, av_decoders::VideoDetails) -> Result<()>,
    mut submit: impl for<'r, 'd> FnMut(usize, PlaneSet<'r>, PlaneSet<'d>) -> Result<R>,
) -> Result<Vec<R>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let output_end = output_start
        .checked_add(selected.len())
        .context("compacted output index overflow")?;

    let reference_details = video_details(reference).context("reference metadata")?;
    let distorted_details = video_details(distorted).context("distorted metadata")?;
    validate(reference_details, distorted_details)?;

    let bit_depth = reference_details.bit_depth;

    // Both sides are opened before any frame is decoded, so a broken graph is
    // reported up front rather than partway through a long pass.
    let mut reference_side = Side::open(reference, bit_depth)?;
    let mut distorted_side = Side::open(distorted, bit_depth)?;
    // A fresh scorer must not rewind a sequential decoder's logical cursor.
    // Seekable sides were actually rewound by open; only sequential sides need
    // the preceding successful scene's known position restored.
    if let Side::Native {
        seekable: false,
        next,
        ..
    } = &mut reference_side
    {
        *next = reference_next;
    }
    if let Side::Native {
        seekable: false,
        next,
        ..
    } = &mut distorted_side
    {
        *next = match indexing {
            OutputIndexing::Compacted => output_start,
            OutputIndexing::Aligned => reference_next,
        };
    }

    check_seekable_selection(&reference_side, &distorted_side, selected, indexing)?;

    let mut results = Vec::with_capacity(selected.len());

    // Pairs are read as they are decoded, so peak memory is the metric's own
    // picture pool plus one pair regardless of clip length; buffering the whole
    // selection instead would scale without bound.
    for (position, &source_index) in selected.iter().enumerate() {
        if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            return Err(Cancelled.into());
        }

        let output_index = match indexing {
            OutputIndexing::Compacted => {
                output_start.checked_add(position).context("compacted output index overflow")?
            },
            OutputIndexing::Aligned => source_index,
        };

        reference_side.advance(source_index)?;
        distorted_side.advance(output_index).with_context(|| {
            // A compacted output holds exactly the selection; an aligned one
            // is the full-length encode.
            let output_frames = match indexing {
                OutputIndexing::Compacted if output_start == 0 => Some(output_end),
                OutputIndexing::Compacted => distorted_details.total_frames,
                OutputIndexing::Aligned => distorted_details.total_frames,
            };
            let output = output_frames.map_or_else(
                || String::from("the encoded output"),
                |frames| format!("the {frames} frame encoded output"),
            );
            format!("failed to reach frame {output_index} of {output}")
        })?;

        // Both sides are read inside nested calls so each frame's planes stay
        // valid until the metric has copied them. The decoders are never
        // borrowed in the same expression, because a VapourSynth node
        // borrows its decoder.
        results.push(
            distorted_side.with_planes(output_index, |distorted_planes| {
                reference_side.with_planes(source_index, |reference_planes| {
                    submit(position, reference_planes, distorted_planes)
                })
            })?,
        );
    }

    Ok(results)
}

/// Reject a selection that a decoder without seeking cannot serve.
///
/// A side that cannot seek reads forward and discards, so it serves any
/// ascending selection but fails outright on a step backwards. Checking this up
/// front turns a mid-pass decode failure into an error beside the
/// configuration that caused it.
///
/// # Errors
///
/// Returns an error if a non-seekable side would have to read a frame before
/// the one it last read.
#[inline]
fn check_seekable_selection(
    reference: &Side<'_>,
    distorted: &Side<'_>,
    selected: &[usize],
    indexing: OutputIndexing,
) -> Result<()> {
    // Each side is addressed in its own index space. A compacted output is read
    // by position, so it ascends by construction; only an aligned output
    // follows the reference into the source frame numbers and can step
    // back.
    let backwards = (!reference.is_seekable() && steps_backwards(selected.iter().copied()))
        || (!distorted.is_seekable()
            && indexing == OutputIndexing::Aligned
            && steps_backwards(selected.iter().copied()));

    if backwards {
        bail!(
            "cannot score this selection with a decoder that cannot seek: it steps backwards, and \
             a decoder without seeking can only read forward. Use a seekable decoder, or select \
             frames in ascending order."
        );
    }

    Ok(())
}

/// Whether `indices` ever decrease.
///
/// This is the condition [`Side::advance`] enforces: it decodes forward and
/// discards, so a step backwards is the one thing it cannot serve.
#[inline]
fn steps_backwards(indices: impl IntoIterator<Item = usize>) -> bool {
    indices
        .into_iter()
        // The window type is spelled out because `indices` is generic here, so
        // there is no concrete iterator for the item type to be inferred from.
        .tuple_windows::<(usize, usize)>()
        .any(|(previous, current)| current < previous)
}

/// Reject pairs whose geometry or bit depth disagree.
///
/// Every metric defined on a full-frame comparison needs both clips to agree on
/// resolution and bit depth, so this is offered as the shared `validate` step.
///
/// # Errors
///
/// Returns an error if the two clips differ in resolution or bit depth.
#[inline]
pub fn check_geometry(
    reference: av_decoders::VideoDetails,
    distorted: av_decoders::VideoDetails,
) -> Result<()> {
    if reference.width != distorted.width || reference.height != distorted.height {
        bail!(
            "reference is {}x{} but the encoded output is {}x{}",
            reference.width,
            reference.height,
            distorted.width,
            distorted.height
        );
    }

    if reference.bit_depth != distorted.bit_depth {
        bail!(
            "reference is {}-bit but the encoded output is {}-bit",
            reference.bit_depth,
            distorted.bit_depth
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The resumable private driver admits an explicit compacted offset that
    // the public probe entry point always sets to zero.
    mod internal_integration {
        use super::*;

        #[test]
        fn compacted_offset_overflow_rejects_before_metadata_or_reads() {
            let script = "import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=32,length=1,format=vs.YUV420P8).set_output()";
            let mut a =
                av_decoders::Decoder::from_script(script, Default::default()).expect("reference");
            let mut b =
                av_decoders::Decoder::from_script(script, Default::default()).expect("distorted");
            let result = probe_selected_frames_from::<()>(
                &mut a,
                &mut b,
                &[0],
                OutputIndexing::Compacted,
                usize::MAX,
                0,
                None,
                |_, _| panic!("overflow reached metadata validation"),
                |_, _, _| panic!("overflow read a pair"),
            );
            assert!(result.expect_err("checked output range").to_string().contains("overflow"));
        }
    }

    #[test]
    fn native_planes_carry_actual_u16_allocation_spans() {
        let frame = av_decoders::v_frame::frame::FrameBuilder::new(
            6,
            4,
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv420,
            10,
        )
        .build::<u16>()
        .expect("native frame");
        let planes = native_planes(&frame).expect("native descriptors");
        for (index, plane) in planes.iter().enumerate() {
            let storage = frame.plane(index).expect("three planes");
            assert_eq!(plane.readable_len(), std::mem::size_of_val(storage.data()));
            assert_eq!(plane.stride() as usize, storage.geometry().stride() * 2);
            assert_eq!(plane.data_origin(), storage.geometry().data_origin() * 2);
        }
    }

    /// A compacted output holds only the selected frames, so it is read by
    /// position within the selection while the reference is read by source
    /// frame number.
    #[test]
    fn compacted_output_index_uses_selection_position() {
        // A `Subset{Middle, 11}` probe of a scene starting at frame 500.
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                position,
                "the compacted output must be read from the matching position"
            );
        }
    }

    #[test]
    fn sparse_compacted_output_indices_stay_within_selection_length() {
        // A compacted output holds exactly `selected.len()` frames, so every
        // output index must be below that. Seeking by source index would exceed
        // it and fail outright.
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            let output = output_index(position, source_index, OutputIndexing::Compacted);
            assert!(
                output < selected.len(),
                "output index {output} is out of range for a {}-frame selection",
                selected.len()
            );
        }
    }

    #[test]
    fn unsorted_compacted_output_indices_follow_selection_position() {
        let selected = [7usize, 3, 900, 12];

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                position
            );
        }
    }

    /// A completed encode is full length, so both sides are addressed by the
    /// same index. This is what the quality-check call site passes.
    #[test]
    fn an_aligned_output_is_addressed_by_source_frame() {
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Aligned),
                source_index,
                "an aligned output uses the same index as the source"
            );
        }
    }

    /// The two index spaces coincide only for a contiguous selection starting
    /// at zero, which is exactly the `Whole` strategy.
    #[test]
    fn both_indexings_agree_only_for_a_whole_selection() {
        let whole: Vec<usize> = (0..32).collect();
        for (position, &source_index) in whole.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                output_index(position, source_index, OutputIndexing::Aligned),
                "a Whole selection is the same in either index space"
            );
        }

        let sparse: Vec<usize> = (10..40).step_by(10).collect();
        for (position, &source_index) in sparse.iter().enumerate() {
            assert_ne!(
                output_index(position, source_index, OutputIndexing::Compacted),
                output_index(position, source_index, OutputIndexing::Aligned),
                "a sparse selection must not be read the same way in both index spaces"
            );
        }
    }

    #[test]
    fn single_frame_output_index_is_zero_when_compacted_and_source_index_when_aligned() {
        // A compacted output holds one frame, so it is position zero.
        assert_eq!(output_index(0, 42, OutputIndexing::Compacted), 0);
        // An aligned output sits at the source index instead.
        assert_eq!(output_index(0, 42, OutputIndexing::Aligned), 42);
    }

    /// A side that cannot seek reads forward, so an ascending selection works
    /// from any starting frame — only a step backwards is impossible.
    /// Checking this up front turns a mid-pass failure into an error beside
    /// the configuration.
    #[test]
    fn backwards_step_detection_rejects_descending_frame_indices() {
        // An ascending run from a nonzero start is servable: the frames before
        // it are decoded and discarded, which is exactly what `advance` does.
        assert!(!steps_backwards([500usize, 501, 502].iter().copied()));
        // A single frame has no pair to step back between.
        assert!(!steps_backwards(std::iter::once(7usize)));

        // A step backwards cannot be served, whichever side asks for it.
        for selection in [&[10usize, 3, 12][..], &[5usize, 4][..]] {
            assert!(
                steps_backwards(selection.iter().copied()),
                "the unsorted selection {selection:?} must be rejected"
            );
        }
    }
}
