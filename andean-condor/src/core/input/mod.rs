//! A decoded video input, and the backends that can produce it.
//!
//! An [`Input`] is decoded natively through FFMS2 or read through VapourSynth.
//! Each has its own filter vocabulary, so building inputs lives in the
//! [`ffms2`] and [`vapoursynth`] submodules, while reading frames back — as
//! y4m or as the raw planes of a [`FrameFeed`] — lives here.

use std::{collections::HashMap, io::Cursor, path::PathBuf};

// `::vapoursynth` is the crate, not this module's `vapoursynth` submodule.
use crate::utils::cancellation::Cancellation;
use ::vapoursynth::node::Node;
use anyhow::Result;
use av_decoders::{Decoder, VideoDetails, v_frame::chroma::ChromaSubsampling};
pub use av_decoders::{DecoderError, ModifyNode};

pub use crate::core::input::ffms2_filter::{ChromaSampling, Ffms2Filter};
use crate::{
    core::input::clip_info::ClipInfo,
    models::input::{
        ImportMethod, Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};

mod ffms2;
mod frame_feed;
mod vapoursynth;
#[cfg(all(test, target_os = "linux"))]
pub(crate) use vapoursynth::indexing;

pub use self::frame_feed::{FrameFeed, RawFrame, RawPlane};

pub mod clip_info;
pub mod color_range;
pub mod ffms2_filter;
pub mod pixel_format;

/// Reports backend indexing progress as `(current, total)` units — a whole
/// percentage for FFMS2, whatever a subprocess's stdout counts for DGDecNV.
pub type IndexProgress<'a> = &'a mut dyn FnMut(u64, u64);

/// The stage of opening an input, as reported by [`Input::ensure_indexed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenProgress {
    /// FFMS2 is indexing the source, `current` percent of `total`. FFMS2
    /// reports whole percentages, so `total` is always `100`.
    Indexing { current: u64, total: u64 },
    /// An index already existed and the input is being opened. No finer
    /// progress is available — VapourSynth indexes inside its source plugin,
    /// out of reach of this process.
    Opening,
}

pub enum Input {
    Video {
        path: PathBuf,
        import_method: ImportMethod,
        filters: Vec<Ffms2Filter>,
        /// The stream's format before `filters` is applied, which is what a
        /// filter's unset fields resolve against. `decoder`'s details describe
        /// the *filtered* output.
        source_details: VideoDetails,
        decoder: Decoder,
        clip_info: Option<ClipInfo>,
    },
    VapourSynth {
        path: PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path: Option<PathBuf>,
        filters: Vec<VapourSynthFilter>,
        decoder: Decoder,
        clip_info: Option<ClipInfo>,
    },
    VapourSynthScript {
        source: VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index: u8,
        filters: Vec<VapourSynthFilter>,
        stream_concurrently: bool,
        decoder: Decoder,
        clip_info: Option<ClipInfo>,
    },
}

impl Input {
    /// The model this input was built from, for storing and re-opening.
    #[inline]
    pub fn as_data(&self) -> InputModel {
        match self {
            Input::Video {
                path,
                import_method,
                filters,
                ..
            } => InputModel::Video {
                path: path.clone(),
                import_method: import_method.clone(),
                filters: filters.clone(),
            },
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                filters,
                ..
            } => InputModel::VapourSynth {
                path: path.clone(),
                import_method: import_method.clone(),
                cache_path: cache_path.clone(),
                filters: filters.clone(),
            },
            Input::VapourSynthScript {
                source,
                variables,
                index,
                filters,
                stream_concurrently,
                ..
            } => InputModel::VapourSynthScript {
                source: source.clone(),
                variables: variables.clone(),
                index: *index,
                filters: filters.clone(),
                stream_concurrently: *stream_concurrently,
            },
        }
    }

    /// Checks that `data` names an input this build can open.
    #[inline]
    pub fn validate(data: &InputModel) -> Result<()> {
        match data {
            InputModel::Video { path, .. } => ffms2::validate(path),
            // TODO: Check if VapourSynth + plugin is installed and static cache it
            InputModel::VapourSynth { path, .. } => vapoursynth::validate(path),
            InputModel::VapourSynthScript { source, .. } => vapoursynth::validate_script(source),
        }
    }

    /// Opens the input described by `data`.
    #[inline]
    pub fn from_data(data: &InputModel) -> Result<Self> {
        match data {
            InputModel::Video { .. } => ffms2::from_video(data),
            InputModel::VapourSynth { .. } | InputModel::VapourSynthScript { .. } => {
                Self::from_vapoursynth(data, None)
            },
        }
    }

    /// Opens a native FFMS2 input, applying its filters.
    #[inline]
    pub fn from_video(data: &InputModel) -> Result<Self> {
        ffms2::from_video(data)
    }

    /// Opens a VapourSynth input, chaining `modify_node` and then the input's
    /// own filters onto the source node.
    #[inline]
    pub fn from_vapoursynth(
        data: &InputModel,
        modify_node: Option<av_decoders::ModifyNode>,
    ) -> Result<Self> {
        vapoursynth::from_vapoursynth(data, modify_node, &Cancellation::default())
    }

    /// Opens a VapourSynth input through a generated script that bakes the
    /// input's filters in, for filters `invoke_plugin_function` cannot reach.
    #[inline]
    pub fn from_vapoursynth_scripted(data: &InputModel) -> Result<Self> {
        vapoursynth::from_vapoursynth_scripted(data, &Cancellation::default())
    }

    /// Opens the input, reporting FFMS2 or DGDecNV indexing progress when
    /// available, then [`OpenProgress::Opening`]. Use the same job token for
    /// opening and later stages. Graceful cancellation lets an admitted index
    /// finish; force can stop the indexer but cannot preempt a hung native plugin.
    #[inline]
    pub fn ensure_indexed(
        data: &InputModel,
        mut progress: impl FnMut(OpenProgress),
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check_admission()?;
        let cancellation = &cancellation.active();
        Input::validate(data)?;
        match data {
            InputModel::Video {
                path,
                import_method,
                ..
            } => {
                let ImportMethod::FFMS2 { cache_path, .. } = import_method;
                ffms2::index_video(
                    path,
                    cache_path.as_deref(),
                    Some(&mut |current, total| progress(OpenProgress::Indexing { current, total })),
                    cancellation,
                )?;
            },
            InputModel::VapourSynth { .. } => {
                vapoursynth::index_source(data, &mut progress, cancellation)?;
            },
            InputModel::VapourSynthScript { .. } => {},
        }
        cancellation.check_forced()?;
        progress(OpenProgress::Opening);
        cancellation.check_forced()?;
        let input = match data {
            InputModel::Video { .. } => ffms2::from_video(data)?,
            InputModel::VapourSynth { .. } | InputModel::VapourSynthScript { .. } => {
                vapoursynth::from_vapoursynth(data, None, cancellation)?
            },
        };
        cancellation.check_forced()?;
        Ok(input)
    }

    /// Reopens a natively-decoded input through VapourSynth, translating its
    /// native filters first so both paths convert the clip the same way.
    /// Already-VapourSynth inputs return `None`.
    #[inline]
    pub fn as_vapoursynth_script(&mut self) -> Result<Option<Self>> {
        vapoursynth::as_vapoursynth_script(self)
    }

    /// The input's decoder.
    #[inline]
    pub fn decoder(&mut self) -> &mut Decoder {
        match self {
            Input::Video { decoder, .. }
            | Input::VapourSynth { decoder, .. }
            | Input::VapourSynthScript { decoder, .. } => decoder,
        }
    }

    /// A sequential feed of the raw frames in `[first, end)`, addressed by
    /// absolute index through whichever backend decodes this input. Frames
    /// borrow the feed, so only one decode is resident at a time and no plane
    /// is copied into an av-decoders
    /// [`Frame`](av_decoders::v_frame::frame::Frame) first.
    #[inline]
    pub fn frame_feed(&mut self, first: usize, end: usize) -> Result<FrameFeed<'_>> {
        FrameFeed::new(self, first, end)
    }

    /// Opens an indexed Y4M reader, retaining one filtered output node for
    /// this feed. Native input borrows its existing decoder without reopening.
    /// Drop the reader before changing the input or its filter settings.
    #[inline]
    pub fn frame_reader(&mut self) -> Result<FrameReader<'_>> {
        Ok(FrameReader {
            kind: match self {
                Input::Video { decoder, .. } => FrameReaderKind::NativeBorrowed(decoder),
                Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                    FrameReaderKind::VapourSynth(vapoursynth::output_node(decoder)?)
                },
            },
        })
    }

    /// Describes the clip, caching the result on the input.
    #[inline]
    pub fn clip_info(&mut self) -> Result<ClipInfo> {
        let slot = match self {
            Input::Video {
                decoder, clip_info, ..
            } => {
                if clip_info.is_none() {
                    *clip_info = Some(ffms2::clip_info(decoder)?);
                }
                clip_info.as_ref()
            },
            Input::VapourSynth {
                decoder, clip_info, ..
            }
            | Input::VapourSynthScript {
                decoder, clip_info, ..
            } => {
                if clip_info.is_none() {
                    *clip_info = Some(vapoursynth::clip_info(decoder)?);
                }
                clip_info.as_ref()
            },
        };
        Ok(*slot.expect("ClipInfo is Some"))
    }

    /// The `C` tag in a y4m header, describing bit depth and chroma layout.
    /// Taken from the decoder rather than `ClipInfo`, so it describes the
    /// frames actually written.
    #[inline]
    pub fn y4m_chroma_tag(bit_depth: usize, chroma: ChromaSubsampling) -> String {
        let chroma_str = match chroma {
            ChromaSubsampling::Monochrome => "mono",
            ChromaSubsampling::Yuv420 => "420",
            ChromaSubsampling::Yuv422 => "422",
            ChromaSubsampling::Yuv444 => "444",
        };
        match chroma {
            // High-depth monochrome needs the depth suffix, but no `p` marker.
            ChromaSubsampling::Monochrome if bit_depth > 8 => format!("{chroma_str}{bit_depth}"),
            _ if bit_depth > 8 => format!("{chroma_str}p{bit_depth}"),
            _ => chroma_str.to_owned(),
        }
    }

    /// The y4m stream header for `frames` frames.
    #[inline]
    pub fn y4m_header(&mut self, frames: Option<usize>) -> Result<String> {
        let clip_info = self.clip_info()?;
        let details = self.decoder().get_video_details();

        Ok(format!(
            "YUV4MPEG2 C{} W{} H{} F{}:{} Ip A0:0{}\n",
            Input::y4m_chroma_tag(details.bit_depth, details.chroma_sampling),
            clip_info.resolution.0,
            clip_info.resolution.1,
            clip_info.frame_rate.numer(),
            clip_info.frame_rate.denom(),
            frames.map_or_else(String::new, |frames| format!(" XLENGTH={frames}"))
        ))
    }

    /// Writes frame `index` as a complete y4m frame. Native input packs
    /// FFMS2's planes directly, without building an av-decoders `Frame`.
    #[inline]
    pub fn y4m_frame(&mut self, index: usize) -> Result<Cursor<Vec<u8>>> {
        self.frame_reader()?.y4m_frame(index)
    }

    /// Sends Y4M frames in order, using the backend's default request window.
    /// Drops the supplied sender on success or failure. Force stops new
    /// requests and blocked sends; outstanding native callbacks must finish.
    #[inline]
    pub fn y4m_frames(
        &mut self,
        sender: crossbeam_channel::Sender<Cursor<Vec<u8>>>,
        indices: &[usize],
        cancellation: &Cancellation,
    ) -> Result<()> {
        let result = self.frame_reader()?.y4m_frames(
            &sender,
            indices,
            frame_feed::request_window(),
            cancellation,
        );
        drop(sender);
        result
    }

    /// Whether [`Input::frame_sources`] can serve several readers at once
    /// without them slowing each other down. False for DGDecNV, which is
    /// unmeasured. A script streams concurrently unless the user turned it
    /// off, since its source plugin is unknown and may be worth measuring.
    #[inline]
    #[must_use]
    pub fn streams_concurrently(&self) -> bool {
        match self {
            Input::Video { .. } => true,
            Input::VapourSynth { import_method, .. } => {
                vapoursynth::streams_concurrently(import_method)
            },
            Input::VapourSynthScript {
                stream_concurrently,
                ..
            } => *stream_concurrently,
        }
    }

    /// Creates `count` independent [`FrameSource`]s, so frames can be streamed
    /// on several threads at once. Native sources each open their own
    /// decoder; VapourSynth sources share one plugin instance, so `count`
    /// may exceed the readers an instance can serve.
    #[inline]
    pub fn frame_sources(&mut self, count: usize) -> Result<Vec<FrameSource<'_>>> {
        let data = self.as_data();
        match self {
            Input::Video { .. } => Ok(std::iter::repeat_with(|| FrameSource {
                kind: FrameSourceKind::Native(data.clone()),
            })
            .take(count)
            .collect()),
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                decoder,
                ..
            } => vapoursynth::frame_sources(
                count,
                path,
                cache_path.as_deref(),
                import_method,
                decoder,
            )
            .map(|nodes| {
                nodes
                    .into_iter()
                    .map(|node| FrameSource {
                        kind: FrameSourceKind::VapourSynth(node),
                    })
                    .collect()
            }),
            Input::VapourSynthScript { decoder, .. } => {
                // Every source shares the script's output node.
                let node = vapoursynth::output_node(decoder)?;
                Ok(std::iter::repeat_with(|| FrameSource {
                    kind: FrameSourceKind::VapourSynth(node.clone()),
                })
                .take(count)
                .collect())
            },
        }
    }
}

/// A handle for streaming an [`Input`]'s frames on another thread. Move it to
/// the reading thread and call [`FrameSource::open`] there.
pub struct FrameSource<'a> {
    kind: FrameSourceKind<'a>,
}

enum FrameSourceKind<'a> {
    VapourSynth(Node<'a>),
    /// Native decoders are not thread-safe, so each source opens its own
    /// decoder on the thread that reads from it.
    Native(InputModel),
}

impl<'a> FrameSource<'a> {
    /// Opens the source for reading on the current thread.
    #[inline]
    pub fn open(self) -> Result<FrameReader<'a>> {
        Ok(FrameReader {
            kind: match self.kind {
                FrameSourceKind::VapourSynth(node) => FrameReaderKind::VapourSynth(node),
                FrameSourceKind::Native(data) => {
                    FrameReaderKind::Native(Box::new(Input::from_data(&data)?))
                },
            },
        })
    }
}

/// Reads indexed frames on a single thread, retaining the feed's output node
/// or native decoder. The graph is immutable for the reader's lifetime.
pub struct FrameReader<'a> {
    kind: FrameReaderKind<'a>,
}

enum FrameReaderKind<'a> {
    VapourSynth(Node<'a>),
    Native(Box<Input>),
    NativeBorrowed(&'a mut Decoder),
}

impl FrameReader<'_> {
    /// Sends Y4M frames in order without consuming the sender. VapourSynth
    /// keeps at most `window.max(1)` frames in flight. Force stops requests and sends.
    #[inline]
    pub fn y4m_frames(
        &mut self,
        sender: &crossbeam_channel::Sender<Cursor<Vec<u8>>>,
        indices: &[usize],
        window: usize,
        cancellation: &Cancellation,
    ) -> Result<()> {
        if let FrameReaderKind::VapourSynth(node) = &self.kind {
            return vapoursynth::y4m_frames(node, sender, indices, window.max(1), cancellation);
        }
        for &index in indices {
            cancellation.check_forced()?;
            let frame = self.y4m_frame(index)?;
            cancellation.send(sender, frame)?;
        }
        Ok(())
    }
    /// Reads one complete Y4M frame at an absolute index. Calls may repeat or
    /// seek to any index; a failed read returns no partial frame bytes.
    #[inline]
    pub fn y4m_frame(&mut self, index: usize) -> Result<Cursor<Vec<u8>>> {
        let mut stream = Cursor::new(Vec::new());
        match &mut self.kind {
            FrameReaderKind::VapourSynth(node) => {
                vapoursynth::y4m_frame(node, index, stream.get_mut())?;
            },
            FrameReaderKind::Native(input) => {
                ffms2::y4m_frame(input.decoder(), index, stream.get_mut())?;
            },
            FrameReaderKind::NativeBorrowed(decoder) => {
                ffms2::y4m_frame(decoder, index, stream.get_mut())?;
            },
        }
        Ok(stream)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("No video file found at {0}")]
    VideoFileNotFound(PathBuf),
    #[error("No VapourSynth script file found at {0}")]
    VapourSynthScriptNotFound(PathBuf),
    #[error("File {0} is not a video file")]
    NotAVideoFile(PathBuf),
    #[error("File {0} is not a VapourSynth script")]
    NotAVapourSynthScript(PathBuf),
}

#[cfg(test)]
mod tests;
