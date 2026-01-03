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
pub struct VideoSource {
    /// The source filename.
    ///
    /// Note that image sequences also can be opened by using %d or %03d for
    /// zero padded numbers. Sequences may start at any number between 0 and 4
    /// unless otherwise specified with start_number. It's also possible to pass
    /// urls and other ffmpeg protocols like concat.
    pub source: PathBuf,

    /// Either a positive number starting from 0 specifying the absolute track
    /// number or a negative number to select the nth video track. Throws an
    /// error on wrong type or no matching track.
    ///
    /// Defaults to `-1`
    pub track: Option<i32>,

    /// Allow format changes in the output for video. To only allow fixed format
    /// output pass 0 or greater to choose the nth encountered format as the
    /// output format. Any frames not matching the chosen format are dropped. If
    /// the file is constant format (most are) this setting does nothing.
    ///
    /// Defaults to `-1`
    pub variable_format: Option<i32>,

    /// Convert the source material to constant framerate. Cannot be combined
    /// with rff.
    ///
    /// Defaults to `-1`
    pub fpsnum: Option<i32>,

    /// Convert the source material to constant framerate. Used in conjunction
    /// with fpsnum.
    ///
    /// Defaults to `-1`
    pub fpsden: Option<i32>,

    /// Apply RFF flags to the video. If the video doesn't have or use RFF flags
    /// the output is unchanged compare to when the option is disabled. Cannot
    /// be combined with fpsnum.
    ///
    /// Defaults to `false`
    pub rff: Option<bool>,

    /// Number of threads to use for decoding. Pass 0 to autodetect.
    ///
    /// Defaults to `0`
    pub threads: Option<i32>,

    /// Number of frames before the requested frame to cache when seeking.
    ///
    /// Defaults to `20`
    pub seekpreroll: Option<u32>,

    /// Option passed to the FFmpeg mov demuxer.
    ///
    /// Defaults to `false`
    pub enable_drefs: Option<bool>,

    /// Option passed to the FFmpeg mov demuxer.
    ///
    /// Defaults to `false`
    pub use_absolute_path: Option<bool>,

    /// * 0 = Never read or write index to disk
    /// * 1 = Always try to read index but only write index to disk when it will
    ///   make a noticeable difference on subsequent runs and store index files
    ///   in a subtree of `cachepath`
    /// * 2 = Always try to read and write index to disk and store index files
    ///   in a subtree of `cachepath`
    /// * 3 = Always try to read index but only write index to disk when it will
    ///   make a noticeable difference on subsequent runs and store index files
    ///   with `cachepath` used as the base filename with track number and index
    ///   extension automatically appended
    /// * 4 = Always try to read and write index to disk and store index files
    ///   with `cachepath` used as the base filename with track number and index
    ///   extension automatically appended
    ///
    /// Defaults to `1`
    pub cachemode: Option<u32>,

    /// The path where cache files are written.
    ///
    /// Note that the actual index files are written into subdirectories using
    /// based on the source location.
    ///
    /// Defaults to %LOCALAPPDATA% on Windows and $XDG_CACHE_HOME/bsindex if set
    /// otherwise ~/bsindex on other operation systems in mode `1` and `2`. For
    /// mode `3` and `4` it defaults to source.
    pub cachepath: Option<PathBuf>,

    /// Maximum internal cache size in MB.
    ///
    /// Defaults to `100`
    pub cachesize: Option<u32>,

    /// The interface to use for hardware decoding.
    ///
    /// Depends on OS and hardware. On windows d3d11va, cuda and vulkan (H264,
    /// HEVC and AV1) are probably the ones most likely to work.
    ///
    /// Defaults to CPU decoding. Will throw errors for formats where hardware
    /// decoding isn't possible.
    pub hwdevice: Option<String>,

    /// The number of additional frames to allocate when hwdevice is set.
    ///
    /// The number required is unknowable and found through trial and error. The
    /// default may be too high or too low. FFmpeg unfortunately is this badly
    /// designed.
    ///
    /// Defaults to `9`
    pub extrahwframes: Option<i32>,

    /// Writes a timecode v2 file with all frame times to the file if specified.
    ///
    /// Note that this option will produce an error if any frame has an unknown
    /// timestamp which would result in an invalid timecode file.
    pub timecodes: Option<PathBuf>,

    /// The first number of image sequences.
    pub start_number: Option<u32>,

    /// The view id to output, this is currently only used for some mv-hevc
    /// files and is quite rare.
    ///
    /// Defaults to `0`
    pub viewid: Option<i32>,

    /// Print indexing progress as VapourSynth information level log messages.
    ///
    /// Defaults to `true`
    pub showprogress: Option<bool>,

    /// The maximum number of decoder instances kept around, defaults to 4 but
    /// when decoding high resolution content it may be beneficial to reduce it
    /// to 1 to reduce peak memory usage. For example 4k h264 material will use
    /// approximately 250MB of ram in addition to the specified cache size for
    /// decoder instance. Passing a number outside the 1-4 range will set it to
    /// the biggest number supported.
    ///
    /// Defaults to `4`
    pub maxdecoders: Option<u32>,

    /// Automatically fall back to CPU decoding if hardware decoding can't be
    /// used for the current video track when hwdevice is set. Note that the
    /// fallback only happens when a hardware decoder is unavailable and not on
    /// any other category of error such as hwdevice having an invalid value.
    ///
    /// Defaults to `true`
    pub hwfallback: Option<bool>,

    /// Returns an additional array of all frame timestamps and its timebase in
    /// timebasenum and timebaseden containing all frame times addition to the
    /// video clip.
    ///
    /// Note that unknown timestamps can be set to AV_NOPTS_VALUE. Cannot be
    /// combined with rff and fpsnum modes.
    ///
    /// Defaults to `false`
    pub exporttimestamps: Option<bool>,
}

impl PluginFunction for VideoSource {
    const PLUGIN_NAME: &'static str = "BestSource";
    const PLUGIN_ID: &'static str = "com.vapoursynth.bestsource";
    const FUNCTION_NAME: &'static str = "VideoSource";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("source", &ValueType::Data)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("cachepath", &ValueType::Data),
        ("hwdevice", &ValueType::Data),
        ("timecodes", &ValueType::Data),
        ("track", &ValueType::Int),
        ("variableformat", &ValueType::Int),
        ("fpsnum", &ValueType::Int),
        ("fpsden", &ValueType::Int),
        ("rff", &ValueType::Int),
        ("threads", &ValueType::Int),
        ("seekpreroll", &ValueType::Int),
        ("enable_drefs", &ValueType::Int),
        ("use_absolute_path", &ValueType::Int),
        ("cachemode", &ValueType::Int),
        ("cachesize", &ValueType::Int),
        ("extrahwframes", &ValueType::Int),
        ("start_number", &ValueType::Int),
        ("viewid", &ValueType::Int),
        ("showprogress", &ValueType::Int),
        ("maxdecoders", &ValueType::Int),
        ("hwfallback", &ValueType::Int),
        ("exporttimestamps", &ValueType::Int),
    ];
}

impl VideoSource {
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
            absolute(self.source.clone()).map_err(|_| VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  "Failed to get absolute path".to_owned(),
            })?;
        let absolute_cachepath = if let Some(cachepath) = &self.cachepath {
            let absolute_cachefile_path =
                absolute(cachepath).map_err(|_| VapourSynthError::PluginFunctionError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    function: Self::FUNCTION_NAME.to_owned(),
                    message:  "Failed to get absolute path".to_owned(),
                })?;
            Some(absolute_cachefile_path)
        } else {
            None
        };
        let absolute_timecodes = if let Some(timecodes) = &self.timecodes {
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
                "cachepath",
                absolute_cachepath.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
            (
                "hwdevice",
                self.hwdevice.clone().map(|s| s.into_bytes()).as_deref(),
            ),
            (
                "timecodes",
                absolute_timecodes.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("track", self.track),
            ("variableformat", self.variable_format),
            ("fpsnum", self.fpsnum),
            ("fpsden", self.fpsden),
            ("rff", self.rff.map(|b| b as i32)),
            ("threads", self.threads),
            ("seekpreroll", self.seekpreroll.map(|i| i as i32)),
            ("enable_drefs", self.enable_drefs.map(|b| b as i32)),
            (
                "use_absolute_path",
                self.use_absolute_path.map(|b| b as i32),
            ),
            ("cachemode", self.cachemode.map(|i| i as i32)),
            ("cachesize", self.cachesize.map(|i| i as i32)),
            ("extrahwframes", self.extrahwframes),
            ("start_number", self.start_number.map(|i| i as i32)),
            ("viewid", self.viewid),
            ("showprogress", self.showprogress.map(|b| b as i32)),
            ("maxdecoders", self.maxdecoders.map(|i| i as i32)),
            ("hwfallback", self.hwfallback.map(|b| b as i32)),
            ("exporttimestamps", self.exporttimestamps.map(|b| b as i32)),
        ])?;

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for VideoSource {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.bs.VideoSource(source = r\"{}\"",
                self.source.display()
            )?;
            if let Some(cache) = &self.cachepath {
                write!(&mut line, ", cachepath = r\"{}\"", cache.display())?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
