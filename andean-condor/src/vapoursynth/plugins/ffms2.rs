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
pub struct Source {
    /// The source file path to open.
    pub source: PathBuf,

    /// The video track number to open, as seen by the relevant demuxer.
    /// Track numbers start from zero, and are guaranteed to be continous.
    /// `-1` means open the first video track.
    ///
    /// Defaults to `-1`.
    pub track: Option<i32>,

    /// If set to `true` (the default), Source will first check if the cachefile
    /// contains a valid index, and if it does, that index will be used.
    /// If set to `false`, Source will not look for an existing index file.
    ///
    /// Defaults to `true``.
    pub cache: Option<bool>,

    /// The filename of the index file (where the indexing data is saved).
    ///
    /// Defaults to sourcefilename.ffindex where sourcefilename is the name of
    /// the source file in the `source` parameter.
    pub cachefile: Option<PathBuf>,

    /// Controls the framerate of the output; used for VFR to CFR conversions.
    /// if set less than or equal to `0` (the default), the output will contain
    /// the same frames that the input did, and the frame rate reported to
    /// VapourSynth will be set based on the input clip's average frame
    /// duration. if greater than zero, Source will force a constant frame
    /// rate, expressed as a rational number where `fpsnum` is the numerator and
    /// `fpsden` is the denominator.
    ///
    /// Defaults to `-1`.
    pub fpsnum: Option<i32>,
    /// Controls the framerate of the output; used for VFR to CFR conversions.
    /// If `fpsnum` is less than or equal to `0` (the default), the output will
    /// contain the same frames that the input did, and the frame rate
    /// reported to VapourSynth will be set based on the input clip's average
    /// frame duration. If fpsnum is greater than zero, Source will force a
    /// constant frame rate, expressed as a rational number where `fpsnum` is
    /// the numerator and `fpsden` is the denominator.
    ///
    /// See also `fpsnum`.
    ///
    /// Defaults to `1`.
    pub fpsden: Option<i32>,

    /// The number of decoding threads to request from libavcodec.
    /// Setting it to less than or equal to zero means it defaults to the number
    /// of logical CPU's reported by the OS.
    ///
    /// Defaults to `-1`.
    pub threads: Option<i32>,

    /// Filename to write Matroska v2 timecodes for the opened video track to.
    /// If the file exists, it will be truncated and overwritten.
    /// Set to the empty string to disable timecodes writing (this is the
    /// default).
    ///
    /// Defaults to "".
    pub timecodes: Option<PathBuf>,

    /// Controls how seeking is done.
    /// Mostly useful for getting uncooperative files to work.
    ///
    /// Valid modes are:
    /// * `-1`: Linear access without rewind; i.e. will throw an error if each
    ///   successive requested frame number isn't bigger than the last one.
    /// * `0`: Linear access (i.e. if you request frame n without having
    ///   requested all frames from 0 to n-1 in order first, all frames from 0
    ///   to n will have to be decoded before n can be delivered).
    /// * `1`: Safe normal. Bases seeking decisions on the keyframe positions
    ///   reported by libavformat.
    /// * `2`: Unsafe normal. Same as mode 1, but no error will be thrown if the
    ///   exact seek destination has to be guessed.
    /// * `3`: Aggressive. Seeks in the forward direction even if no closer
    ///   keyframe is known to exist. Only useful for testing and containers
    ///   where libavformat doesn't report keyframes properly.
    ///
    /// Defaults to `1`.
    pub seekmode: Option<i32>,

    /// Sets the width, in pixels, of the output video.
    /// Settng to less than or equal to zero means the resolution of the first
    /// decoded video frame is used.
    ///
    /// Defaults to `-1`.
    pub width: Option<i32>,

    /// Sets the height, in pixels, of the output video.
    /// Setting to less than or equal to zero means the resolution of the first
    /// decoded video frame is used.
    ///
    /// Defaults to `-1`.
    pub height: Option<i32>,

    /// The resizing algorithm to use if rescaling the image is necessary.
    /// If the video uses subsampled chroma but your chosen output colorspace
    /// does not, the chosen resizer will be used to upscale the chroma planes,
    /// even if you did not request an image rescaling.
    ///
    /// Available choices are:
    /// * FAST_BILINEAR
    /// * BILINEAR
    /// * BICUBIC
    /// * X
    /// * POINT
    /// * AREA
    /// * BICUBLIN
    /// * GAUSS
    /// * SINC
    /// * LANCZOS
    /// * SPLINE
    ///
    /// Defaults to `BICUBIC`.
    pub resizer: Option<String>,

    /// Convert the output from whatever it was to the given format.
    /// If not specified the best matching output format is used.
    pub format: Option<i32>,

    /// Output the alpha channel as a second clip if it is present in the file.
    /// When set to True an array of two clips will be returned with alpha in
    /// the second one. If there is alpha information present.
    ///
    /// Defaults to `false`.
    pub alpha: Option<bool>,
}

impl PluginFunction for Source {
    const PLUGIN_NAME: &'static str = "ffms2";
    const PLUGIN_ID: &'static str = "com.vapoursynth.ffms2";
    const FUNCTION_NAME: &'static str = "Source";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("source", &ValueType::Data)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("cachefile", &ValueType::Data),
        ("timecodes", &ValueType::Data),
        ("resizer", &ValueType::Data),
        ("track", &ValueType::Int),
        ("cache", &ValueType::Int),
        ("fpsnum", &ValueType::Int),
        ("fpsden", &ValueType::Int),
        ("threads", &ValueType::Int),
        ("seekmode", &ValueType::Int),
        ("width", &ValueType::Int),
        ("height", &ValueType::Int),
        ("format", &ValueType::Int),
        ("alpha", &ValueType::Int),
    ];
}

impl Source {
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
        let absolute_timecodes = if let Some(timecodes) = self.timecodes {
            let absolute_timecodes_path =
                absolute(timecodes).map_err(|_| VapourSynthError::PluginFunctionError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    function: Self::FUNCTION_NAME.to_owned(),
                    message:  "Failed to get absolute path".to_owned(),
                })?;
            Some(absolute_timecodes_path)
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
                "timecodes",
                absolute_timecodes.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
            ("resizer", self.resizer.map(|s| s.into_bytes()).as_deref()),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("track", self.track),
            ("cache", self.cache.map(|b| b as i32)),
            ("fpsnum", self.fpsnum),
            ("fpsden", self.fpsden),
            ("threads", self.threads),
            ("seekmode", self.seekmode),
            ("width", self.width),
            ("height", self.height),
            ("format", self.format),
            ("alpha", self.alpha.map(|b| b as i32)),
        ])?;

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Source {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.ffms2.Source(source = r\"{}\"",
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
