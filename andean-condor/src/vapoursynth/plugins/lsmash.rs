use std::{
    fmt::Write,
    path::{absolute, Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    plugins::PluginFunction,
    script_builder::{
        script::{Imports, Line},
        NodeVariableName,
        VapourSynthPluginScript,
    },
    VapourSynthError,
};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LWLibavSource {
    /// The path of the source file.
    pub source: PathBuf,

    /// The stream index to open in the source file.
    /// The value -1 means trying to get the video stream which has the largest
    /// resolution.
    /// Default: 0
    pub stream_index: Option<i32>,

    /// The number of threads to decode a stream by libavcodec.
    /// The value 0 means the number of threads is determined automatically and
    /// then the maximum value will be up to 16.
    /// Default: 0
    pub threads: Option<u8>,

    /// Create the index file (.lwi) to the same directory as the source file if
    /// set to 1. The index file avoids parsing all frames in the source
    /// file at the next or later access. Parsing all frames is very
    /// important for frame accurate seek.
    /// Default: True
    pub cache: Option<bool>,

    /// The filename of the index file (where the indexing data is saved).
    /// Default: source + ".lwi"
    pub cachefile: Option<PathBuf>,

    /// How to process when any error occurs during decoding a video frame.
    /// - 0 : Normal This mode retries sequential decoding from the next closest
    ///   RAP up to 3 cycles when any decoding error occurs. If all 3 trial
    ///   failed, retry sequential decoding from the last RAP by ignoring
    ///   trivial errors. Still error occurs, then return the last returned
    ///   frame.
    /// - 1 : Unsafe This mode retries sequential decoding from the next closest
    ///   RAP up to 3 cycles when any fatal decoding error occurs. If all 3
    ///   trial failed, then return the last returned frame.
    /// - 2 : Aggressive This mode returns the last returned frame when any
    ///   fatal decoding error occurs.
    pub seek_mode: Option<u8>,

    /// The threshold to decide whether a decoding starts from the closest RAP
    /// to get the requested video frame or doesn't.
    ///
    /// Let's say
    /// - the threshold is T,
    /// - you request to seek the M-th frame called f(M) from the N-th frame
    ///   called f(N). If M > N and M - N <= T, then the decoder tries to get
    ///   f(M) by decoding frames from f(N) sequentially. If M < N or M - N > T,
    ///   then check the closest RAP at the first. After the check, if the
    ///   closest RAP is identical with the last RAP, do the same as the case M
    ///   N and M - N <= T.   Otherwise, the decoder tries to get f(M) by
    ///   decoding frames from the frame which is the closest RAP sequentially.
    /// > Default: 10
    pub seek_threshold: Option<u8>,

    /// Try direct rendering from the video decoder if 'dr' is set to 1 and
    /// 'format' is unspecfied. The output resolution will be aligned to be
    /// mod16-width and mod32-height by assuming two vertical 16x16 macroblock.
    /// For H.264 streams, in addition, 2 lines could be added because of the
    /// optimized chroma MC.
    pub dr: Option<bool>,

    /// Output frame rate numerator for VFR->CFR (Variable Frame Rate to
    /// Constant Frame Rate) conversion. If frame rate is set to a valid
    /// value, the conversion is achieved by padding and/or dropping frames
    /// at the specified frame rate. Otherwise, output frame rate is set to
    /// a computed average frame rate and the output process is performed by
    /// actual frame-by-frame.
    ///
    /// NOTE: You must explicitly set this if the source is an AVI file that
    /// contains null/drop frames that you would like to keep. For example, AVI
    /// files captured using VirtualDub commonly contain null/drop frames
    /// that were inserted during the capture process. Unless you provide
    /// this parameter, these null frames will be discarded, commonly
    /// resulting in loss of audio/video sync.
    pub fpsnum: Option<u64>,

    /// Output frame rate denominator for VFR->CFR (Variable Frame Rate to
    /// Constant Frame Rate) conversion. See 'fpsnum' in details.
    pub fpsden: Option<u64>,

    /// Treat format, width and height of the video stream as variable if set to
    /// 1.
    pub variable: Option<bool>,

    /// Force specified output pixel format if 'format' is specified and
    /// 'variable' is set to `0`. The following formats are available
    /// currently.
    ///
    /// * `YUV420P8`
    /// * `YUV422P8`
    /// * `YUV444P8`
    /// * `YUV410P8`
    /// * `YUV411P8`
    /// * `YUV440P8`
    /// * `YUV420P9`
    /// * `YUV422P9`
    /// * `YUV444P9`
    /// * `YUV420P10`
    /// * `YUV422P10`
    /// * `YUV444P10`
    /// * `YUV420P12`
    /// * `YUV422P12`
    /// * `YUV444P12`
    /// * `YUV420P14`
    /// * `YUV422P14`
    /// * `YUV444P14`
    /// * `YUV420P16`
    /// * `YUV422P16`
    /// * `YUV444P16`
    /// * `Y8`
    /// * `Y16`
    /// * `RGB24`
    /// * `RGB27`
    /// * `RGB30`
    /// * `RGB48`
    /// * `RGB64BE`
    /// * `XYZ12LE`
    pub format: Option<String>,

    /// Reconstruct frames by the flags specified in video stream if set to
    /// non-zero value. If set to 1, and source file requested repeat and
    /// the filter is unable to obey the request, this filter will fail
    /// explicitly to eliminate any guesswork. If set to 2, and source file
    /// requested repeat and the filter is unable to obey the request, silently
    /// returning a VFR clip with a constant (but wrong) fps. Note that this
    /// option is ignored when VFR->CFR conversion is enabled. Note that if
    /// the source is fake interlaced, this option must be set to false.
    pub repeat: Option<u8>,

    /// Which field, top or bottom, is displayed first.
    /// - 0 : Obey source flags
    /// - 1 : TFF i.e. Top -> Bottom
    /// - 2 : BFF i.e. Bottom -> Top
    /// > This option is enabled only if one or more of the following conditions
    /// > is true.
    /// - 'repeat' is set to 1.
    /// - There is a video frame consisting of two separated field coded
    ///   pictures.
    pub dominance: Option<u8>,

    /// Same as 'decoder' of LibavSMASHSource().
    /// This is always unspecified (software decoder) if `rap_verification=1`
    /// during the indexing step.
    pub decoder: Option<String>,

    /// Same as 'prefer_hw' of LibavSMASHSource().
    /// This is always `0` if `rap_verification=1` during the indexing step.
    pub prefer_hw: Option<bool>,

    /// Same as 'ff_loglevel' of LibavSMASHSource().
    pub ff_loglevel: Option<u8>,

    /// Create *.lwi file under this directory with names encoding the full path
    /// to avoid collisions.
    pub cachedir: Option<PathBuf>,

    /// Same as 'ff_options' of LibavSMASHSource().
    pub ff_options: Option<String>,

    /// Whether to verify if the determined RAP by demuxer/parser is valid RAP
    /// (the frame is decoded). This is done in the indexing step.
    /// To avoid the indexing speed penalty set this to `0`.
    /// Switching between `1` and `0` requires manual deletion of the index
    /// file.
    pub rap_verification: Option<bool>,
}

impl PluginFunction for LWLibavSource {
    const PLUGIN_NAME: &'static str = "lsmash";
    const PLUGIN_ID: &'static str = "systems.innocent.lsmas";
    const FUNCTION_NAME: &'static str = "LWLibavSource";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("source", &ValueType::Data)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("cachefile", &ValueType::Data),
        ("cachedir", &ValueType::Data),
        ("format", &ValueType::Data),
        ("decoder", &ValueType::Data),
        ("ff_options", &ValueType::Data),
        ("stream_index", &ValueType::Int),
        ("threads", &ValueType::Int),
        ("cache", &ValueType::Int),
        ("seek_mode", &ValueType::Int),
        ("seek_threshold", &ValueType::Int),
        ("dr", &ValueType::Int),
        ("fpsnum", &ValueType::Int),
        ("fpsden", &ValueType::Int),
        ("variable", &ValueType::Int),
        ("repeat", &ValueType::Int),
        ("dominance", &ValueType::Int),
        ("prefer_hw", &ValueType::Int),
        ("ff_loglevel", &ValueType::Int),
        ("rap_verification", &ValueType::Int),
    ];
}

impl LWLibavSource {
    #[inline]
    pub fn new(source: &Path) -> Self {
        Self {
            source: source.to_path_buf(),
            ..Default::default()
        }
    }

    #[inline]
    pub fn invoke(self, core: CoreRef) -> Result<Node, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        let absolute_source_path =
            absolute(self.source).map_err(|_| VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  "Failed to get absolute path".to_owned(),
            })?;
        let absolute_cachefile = if let Some(cachefile) = self.cachefile {
            let absolute_cachefile_path =
                absolute(cachefile).map_err(|_| VapourSynthError::PluginFunctionError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    function: Self::FUNCTION_NAME.to_owned(),
                    message:  "Failed to get absolute path".to_owned(),
                })?;
            Some(absolute_cachefile_path)
        } else {
            None
        };
        let absolute_cachedir = if let Some(cachedir) = self.cachedir {
            let absolute_cachedir_path =
                absolute(cachedir).map_err(|_| VapourSynthError::PluginFunctionError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    function: Self::FUNCTION_NAME.to_owned(),
                    message:  "Failed to get absolute path".to_owned(),
                })?;
            Some(absolute_cachedir_path)
        } else {
            None
        };

        Self::arguments_set(&mut arguments, vec![
            (
                "source",
                Some(absolute_source_path.display().to_string().as_bytes()),
            ),
            (
                "cachefile",
                absolute_cachefile.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
            (
                "cachedir",
                absolute_cachedir.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
            ("format", self.format.map(|s| s.into_bytes()).as_deref()),
            ("decoder", self.decoder.map(|s| s.into_bytes()).as_deref()),
            (
                "ff_options",
                self.ff_options.map(|s| s.into_bytes()).as_deref(),
            ),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("stream_index", self.stream_index.map(|i| i as i64)),
            ("threads", self.threads.map(|i| i as i64)),
            ("cache", self.cache.map(|i| i as i64)),
            ("seek_mode", self.seek_mode.map(|i| i as i64)),
            ("seek_threshold", self.seek_threshold.map(|i| i as i64)),
            ("dr", self.dr.map(|i| i as i64)),
            ("fpsnum", self.fpsnum.map(|i| i as i64)),
            ("fpsden", self.fpsden.map(|i| i as i64)),
            ("variable", self.variable.map(|i| i as i64)),
            ("repeat", self.repeat.map(|i| i as i64)),
            ("dominance", self.dominance.map(|i| i as i64)),
            ("prefer_hw", self.prefer_hw.map(|i| i as i64)),
            ("ff_loglevel", self.ff_loglevel.map(|i| i as i64)),
            ("rap_verification", self.rap_verification.map(|i| i as i64)),
        ])?;

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for LWLibavSource {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.lsmas.LWLibavSource(source = r\"{}\"",
                self.source.display()
            )?;
            if let Some(cache) = &self.cachefile {
                write!(&mut line, ", cachefile = r\"{}\"", cache.display())?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
