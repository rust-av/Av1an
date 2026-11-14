use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

// Re-exported so callers configuring an input need only this module.
pub use crate::core::input::ffms2_filter::{ChromaSampling, Ffms2Filter};
use crate::vapoursynth::vapoursynth_filters::VapourSynthFilter;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum Input {
    Video {
        path: PathBuf,
        import_method: ImportMethod,
        /// Conversion applied to the decoded frames.
        ///
        /// Native FFMS2 cannot run arbitrary filters; this is the full set of
        /// modifications it supports.
        filters: Vec<Ffms2Filter>,
    },
    VapourSynth {
        path: PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path: Option<PathBuf>,
        /// Filters chained onto the source node, in order.
        filters: Vec<VapourSynthFilter>,
    },
    VapourSynthScript {
        source: VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index: u8,
        /// Filters chained onto the script's output node, in order.
        filters: Vec<VapourSynthFilter>,
        /// Whether several encoders may pull frames from this script at once.
        ///
        /// Streaming frames concurrently bounds memory, because only a fixed
        /// window of frames is kept resident. Setting this to `false` decodes a
        /// whole scene ahead of its encoders instead, trading memory for decode
        /// speed, and is worth trying if the script's source plugin decodes
        /// slowly under concurrent access.
        #[serde(default = "stream_concurrently_default")]
        stream_concurrently: bool,
    },
}

/// An absent `stream_concurrently` means concurrent streaming, so configs
/// written before the field existed keep the bounded-memory behaviour.
#[inline]
#[must_use]
fn stream_concurrently_default() -> bool {
    true
}

impl Input {
    /// The explicitly declared source path; inline code's hidden reads are not enumerable.
    #[inline]
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Video { path, .. } | Self::VapourSynth { path, .. } => Some(path),
            Self::VapourSynthScript {
                source: VapourSynthScriptSource::Path(path),
                ..
            } => Some(path),
            Self::VapourSynthScript {
                source: VapourSynthScriptSource::Text(_),
                ..
            } => None,
        }
    }

    /// This input's VapourSynth filters, or an empty slice for a native input.
    ///
    /// Native filters are a different type, so a native input reports none here
    /// rather than pretending it has VapourSynth filters.
    #[inline]
    #[must_use]
    pub fn vapoursynth_filters(&self) -> &[VapourSynthFilter] {
        match self {
            Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. } => {
                filters
            },
            Input::Video { .. } => &[],
        }
    }

    /// Copies `other`'s filters onto this input.
    ///
    /// Used when an input is rebuilt (a new path or decoder) so its filters are
    /// not lost. A change of input kind converts what it can: switching to a
    /// native input keeps only the filters FFMS2 can express, and switching to
    /// VapourSynth keeps the native ones unchanged, since every native filter
    /// has a VapourSynth equivalent.
    ///
    /// # Errors
    /// Unsupported explicit formats and invalid native dimensions leave this input unchanged.
    #[inline]
    pub fn adopt_filters(&mut self, other: &Input) -> anyhow::Result<()> {
        match (self, other) {
            (
                Input::Video { filters, .. },
                Input::VapourSynth { filters: from, .. }
                | Input::VapourSynthScript { filters: from, .. },
            ) => {
                // The source format is not known here, so an unset field in the
                // native filter means "unchanged" and the decoder resolves it.
                let converted = from
                    .iter()
                    .map(Ffms2Filter::from_vapoursynth_filter)
                    .collect::<anyhow::Result<Vec<_>>>()?;
                *filters = converted.into_iter().flatten().collect();
            },
            (Input::Video { filters, .. }, Input::Video { filters: from, .. }) => {
                *filters = from.clone();
            },
            (
                Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. },
                Input::Video { filters: from, .. },
            ) => {
                // A native filter only ever resizes and converts, which
                // VapourSynth expresses directly.
                *filters = from.iter().flat_map(Ffms2Filter::to_vapoursynth).collect();
            },
            (
                Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. },
                Input::VapourSynth { filters: from, .. }
                | Input::VapourSynthScript { filters: from, .. },
            ) => *filters = from.clone(),
        }
        Ok(())
    }

    /// Replaces this input's filters, converting as needed for its decoder.
    ///
    /// A native FFMS2 input accepts only what FFMS2 can express, so
    /// VapourSynth filters it cannot run are converted where possible (a
    /// `Resize`) and otherwise dropped. [`Input::unsupported_filters`] reports
    /// what was dropped so callers can warn.
    ///
    /// # Errors
    /// Unsupported explicit formats and invalid native dimensions leave the filters unchanged.
    #[inline]
    pub fn set_filters(&mut self, new: Vec<VapourSynthFilter>) -> anyhow::Result<()> {
        match self {
            Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. } => {
                *filters = new;
            },
            Input::Video {
                filters: native, ..
            } => {
                let converted = new
                    .iter()
                    .map(Ffms2Filter::from_vapoursynth_filter)
                    .collect::<anyhow::Result<Vec<_>>>()?;
                *native = converted.into_iter().flatten().collect();
            },
        }
        Ok(())
    }

    /// The filters in `new` this input's decoder cannot run.
    ///
    /// Always empty for a VapourSynth input; for a native input it is every
    /// filter but `Resize` and `OutputFormat`. Invalid conversions are errors,
    /// not filters that can be silently dropped.
    #[inline]
    #[must_use]
    pub fn unsupported_filters(new: &[VapourSynthFilter]) -> Vec<&VapourSynthFilter> {
        new.iter()
            .filter(|filter| matches!(Ffms2Filter::from_vapoursynth_filter(filter), Ok(None)))
            .collect()
    }

    /// Whether any of this input's filters changes the frame count.
    #[inline]
    #[must_use]
    pub fn has_time_altering_filters(&self) -> bool {
        match self {
            Input::Video { .. } => false,
            Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. } => {
                filters.iter().any(VapourSynthFilter::can_alter_time)
            },
        }
    }

    /// Appends `other`'s time-altering filters to this input's filters.
    ///
    /// Used to recover when two inputs of the same source disagree on frame
    /// count because one is missing a trim or splice. A native input has no
    /// time-altering filters, so appending from one is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`InputFilterError::WrongKind`] if `other` is a VapourSynth
    /// input but this one is native.
    #[inline]
    pub fn append_input_filters(&mut self, other: &Input) -> Result<(), InputFilterError> {
        let (Input::VapourSynth { filters, .. } | Input::VapourSynthScript { filters, .. }) = other
        else {
            return Ok(());
        };
        let time_altering = filters
            .iter()
            .filter(|filter| filter.can_alter_time())
            .cloned()
            .collect::<Vec<_>>();
        if time_altering.is_empty() {
            return Ok(());
        }

        match self {
            Input::VapourSynth {
                filters: existing, ..
            }
            | Input::VapourSynthScript {
                filters: existing, ..
            } => {
                existing.extend(time_altering);
                Ok(())
            },
            Input::Video { .. } => {
                // A native input has no time-altering filters to append, and
                // its own are left untouched.
                Err(InputFilterError::WrongKind)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::FFPixelFormat;

    fn vapoursynth_input(filters: Vec<VapourSynthFilter>) -> Input {
        Input::VapourSynth {
            path: "input.mkv".into(),
            import_method: VapourSynthImportMethod::FFMS2 { index: None },
            cache_path: None,
            filters,
        }
    }

    fn video_input(filters: Vec<Ffms2Filter>) -> Input {
        Input::Video {
            path: "input.mkv".into(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters,
        }
    }

    fn native_filters(input: &Input) -> &[Ffms2Filter] {
        let Input::Video { filters, .. } = input else {
            panic!("expected a Video input");
        };
        filters
    }

    fn crop() -> VapourSynthFilter {
        VapourSynthFilter::Crop {
            top: Some(140),
            bottom: None,
            left: None,
            right: None,
        }
    }

    fn resize() -> VapourSynthFilter {
        VapourSynthFilter::Resize {
            scaler: None,
            width: Some(1280),
            height: Some(720),
            format: None,
        }
    }

    fn trim() -> VapourSynthFilter {
        VapourSynthFilter::Trim {
            start: Some(24),
            end: None,
        }
    }

    #[test]
    fn failed_filter_conversion_does_not_replace_existing_filters() {
        for format in [FFPixelFormat::NV12, FFPixelFormat::GBRP, FFPixelFormat::YUV440P] {
            let original = vec![Ffms2Filter::OutputFormat {
                bit_depth: Some(12),
                chroma: None,
                width: None,
                height: None,
            }];
            let invalid = vec![
                resize(),
                VapourSynthFilter::Resize {
                    scaler: None,
                    width: Some(16),
                    height: None,
                    format: Some(format),
                },
            ];
            let mut target = video_input(original.clone());
            let error = target.set_filters(invalid.clone()).expect_err("unsupported format");
            assert!(error.to_string().contains(format.to_pix_fmt_string()));
            assert_eq!(native_filters(&target), original);
            let error = target
                .adopt_filters(&vapoursynth_input(invalid))
                .expect_err("unsupported format");
            assert!(error.to_string().contains(format.to_pix_fmt_string()));
            assert_eq!(native_filters(&target), original);
        }
    }

    #[test]
    fn deferred_filters_roundtrip_without_opening_a_source() {
        let directory = tempfile::tempdir().expect("absent source fixture");
        let path = directory.path().join("must-not-exist.mkv");
        let original = vec![
            Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: None,
                width: None,
                height: None,
            },
            Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: Some(ChromaSampling::Yuv444),
                width: Some(16),
                height: None,
            },
        ];
        let mut native = video_input(original.clone());
        let Input::Video {
            path: native_path, ..
        } = &mut native
        else {
            unreachable!()
        };
        *native_path = path.clone();
        let mut vs = vapoursynth_input(vec![]);
        let Input::VapourSynth { path: vs_path, .. } = &mut vs else {
            unreachable!()
        };
        *vs_path = path;
        vs.adopt_filters(&native).expect("model-only copy");
        let encoded = serde_json::to_string(&vs).expect("deferred JSON");
        let restored: Input = serde_json::from_str(&encoded).expect("deferred JSON roundtrip");
        assert_eq!(
            restored.vapoursynth_filters(),
            &[
                VapourSynthFilter::OutputFormat {
                    bit_depth: Some(10),
                    chroma: None,
                    width: None,
                    height: None
                },
                VapourSynthFilter::OutputFormat {
                    bit_depth: None,
                    chroma: Some(ChromaSampling::Yuv444),
                    width: Some(16),
                    height: None
                }
            ]
        );
        for filter in restored.vapoursynth_filters() {
            assert_eq!(
                filter
                    .to_string()
                    .parse::<VapourSynthFilter>()
                    .expect("deferred display roundtrip"),
                *filter
            );
        }
        native.adopt_filters(&restored).expect("deferred to native");
        assert_eq!(native_filters(&native), original);
        let legacy: VapourSynthFilter = serde_json::from_str(
            r#"{"Resize":{"scaler":"Bicubic","width":null,"height":null,"format":"YUV422P"}}"#,
        )
        .expect("legacy Resize config");
        native.set_filters(vec![legacy]).expect("legacy concrete resize");
        assert_eq!(
            native_filters(&native),
            &[Ffms2Filter::OutputFormat {
                bit_depth: Some(8),
                chroma: Some(ChromaSampling::Yuv422),
                width: None,
                height: None
            }]
        );
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("directory listing").count(),
            0,
            "no decoder/index/config I/O"
        );
    }

    /// Reports what a native input cannot run. A VapourSynth input runs
    /// everything, so a caller only consults this when the input is native.
    #[test]
    fn unsupported_filters_names_only_what_a_native_input_cannot_run() {
        let all = [crop(), resize()];
        let unsupported = Input::unsupported_filters(&all);

        // FFMS2 can express a resize, but not a crop.
        assert_eq!(unsupported.len(), 1, "only the crop is unsupported");
        assert!(matches!(unsupported[0], VapourSynthFilter::Crop { .. }));

        // A trim alters time, which a native input cannot do at all.
        assert_eq!(Input::unsupported_filters(&[trim()]).len(), 1);
    }

    /// Appending carries over only the time-altering filters, so a resize on
    /// the source is not duplicated onto the target.
    #[test]
    fn append_carries_over_only_time_altering_filters() {
        let mut target = vapoursynth_input(Vec::new());
        target
            .append_input_filters(&vapoursynth_input(vec![trim(), resize()]))
            .expect("both inputs are VapourSynth");
        assert_eq!(target.vapoursynth_filters(), &[trim()]);
    }

    /// A source whose filters cannot alter time changes nothing.
    #[test]
    fn append_is_a_no_op_without_time_altering_filters() {
        let mut target = vapoursynth_input(Vec::new());
        target
            .append_input_filters(&vapoursynth_input(vec![resize()]))
            .expect("both inputs are VapourSynth");
        assert!(target.vapoursynth_filters().is_empty());
    }

    /// A native input cannot run a trim, so this is reported rather than
    /// guessed at. Its own filters are left untouched.
    #[test]
    fn appending_time_filters_to_native_input_returns_wrong_kind() {
        let mut native = video_input(Vec::new());
        assert!(matches!(
            native.append_input_filters(&vapoursynth_input(vec![trim()])),
            Err(InputFilterError::WrongKind)
        ));
        assert!(native_filters(&native).is_empty());
    }

    /// Round-tripping a resize through the native representation and back must
    /// preserve it, so switching decoders does not quietly change the output.
    #[test]
    fn a_resize_survives_a_round_trip_through_the_native_form() {
        let native = Ffms2Filter::from_vapoursynth_filter(&resize())
            .expect("valid native conversion")
            .expect("a resize maps to a native filter");
        assert_eq!(
            native.to_vapoursynth().as_slice(),
            &[VapourSynthFilter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(1280),
                height: Some(720),
            }]
        );
    }

    /// Switching back to a VapourSynth input converts the native filters, so
    /// the conversion is not lossy.
    #[test]
    fn adopting_filters_from_a_native_input_converts_them() {
        let ten_bit = Ffms2Filter::OutputFormat {
            bit_depth: Some(10),
            chroma: None,
            width: None,
            height: None,
        };
        let mut target = vapoursynth_input(Vec::new());
        target.adopt_filters(&video_input(vec![ten_bit])).expect("native copy");
        assert_eq!(
            target.vapoursynth_filters(),
            &[VapourSynthFilter::OutputFormat {
                bit_depth: Some(10),
                chroma: None,
                width: None,
                height: None,
            }]
        );
    }

    /// Switching to a native input keeps only what FFMS2 can run. Unlike
    /// `set_filters`, `adopt_filters` does not report what it dropped, so a
    /// caller honouring an explicit `--filters` must warn via
    /// `unsupported_filters`.
    #[test]
    fn adopting_into_a_native_input_drops_what_ffms2_cannot_run() {
        let mut native = video_input(Vec::new());
        native
            .adopt_filters(&vapoursynth_input(vec![crop(), resize()]))
            .expect("supported subset");

        assert_eq!(
            native_filters(&native),
            &[Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(1280),
                height: Some(720),
            }]
        );
    }

    /// Within the same kind of input, adopting is a plain copy.
    #[test]
    fn adopting_within_the_same_kind_keeps_the_filters_verbatim() {
        let mut target = vapoursynth_input(Vec::new());
        target
            .adopt_filters(&vapoursynth_input(vec![crop(), resize()]))
            .expect("VS copy");
        assert_eq!(target.vapoursynth_filters(), &[crop(), resize()]);
    }

    /// `set_filters` replaces rather than appends, and converts for a native
    /// input.
    #[test]
    fn set_filters_replaces_and_converts() {
        let mut native = video_input(vec![Ffms2Filter::OutputFormat {
            bit_depth: Some(12),
            chroma: None,
            width: None,
            height: None,
        }]);
        native.set_filters(vec![resize()]).expect("native resize");
        assert_eq!(
            native_filters(&native),
            &[Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(1280),
                height: Some(720),
            }]
        );

        let mut target = vapoursynth_input(vec![crop()]);
        target.set_filters(vec![trim()]).expect("VS trim");
        assert_eq!(target.vapoursynth_filters(), &[trim()]);
    }

    /// Only a trim alters time, and a native input never does.
    #[test]
    fn only_a_trim_alters_time() {
        assert!(vapoursynth_input(vec![trim()]).has_time_altering_filters());
        assert!(!vapoursynth_input(vec![resize()]).has_time_altering_filters());
        assert!(!video_input(Vec::new()).has_time_altering_filters());
    }

    fn script_json(stream_concurrently: &str) -> String {
        format!(
            r#"{{
                "VapourSynthScript": {{
                    "source": {{ "Path": "script.vpy" }},
                    "variables": {{}},
                    "index": 0,
                    "filters": []
                    {stream_concurrently}
                }}
            }}"#
        )
    }

    fn script_stream_concurrently(input: &Input) -> bool {
        let Input::VapourSynthScript {
            stream_concurrently,
            ..
        } = input
        else {
            panic!("expected a VapourSynthScript input");
        };
        *stream_concurrently
    }

    /// A config written before this field existed must still load, and must get
    /// the bounded-memory behaviour rather than failing to parse.
    #[test]
    fn a_missing_stream_concurrently_defaults_to_streaming() {
        let input: Input = serde_json::from_str(&script_json(""))
            .expect("a script config without the field loads");

        assert!(
            script_stream_concurrently(&input),
            "an absent field means concurrent streaming"
        );
    }

    /// The user has to be able to turn concurrent streaming off.
    #[test]
    fn stream_concurrently_false_survives_a_round_trip() {
        let input: Input = serde_json::from_str(&script_json(r#", "stream_concurrently": false"#))
            .expect("an explicit false loads");

        assert!(
            !script_stream_concurrently(&input),
            "an explicit false disables concurrent streaming"
        );

        let written = serde_json::to_string(&input).expect("a script input serializes");
        let reread: Input = serde_json::from_str(&written).expect("the written config loads");
        assert!(
            !script_stream_concurrently(&reread),
            "false must not be lost when the config is saved and reloaded"
        );
    }

    fn video_cache_path(input: &Input) -> Option<PathBuf> {
        let Input::Video {
            import_method: ImportMethod::FFMS2 { cache_path, .. },
            ..
        } = input
        else {
            panic!("expected a native FFMS2 input");
        };
        cache_path.clone()
    }

    /// A config written before the cache path existed must still load, and must
    /// fall back to FFMS2's default index location next to the source.
    #[test]
    fn a_missing_cache_path_loads_as_none() {
        let input: Input = serde_json::from_str(
            r#"{ "Video": { "path": "input.mkv", "import_method": { "FFMS2": { "index": null } }, "filters": [] } }"#,
        )
        .expect("a native config without a cache path loads");

        assert_eq!(
            video_cache_path(&input),
            None,
            "an absent cache path means FFMS2's default location"
        );
    }

    /// A user-supplied index location has to survive a save and reload.
    #[test]
    fn a_supplied_cache_path_survives_a_round_trip() {
        let mut input = video_input(Vec::new());
        let Input::Video { import_method, .. } = &mut input else {
            panic!("expected a native input");
        };
        let ImportMethod::FFMS2 { cache_path, .. } = import_method;
        *cache_path = Some(PathBuf::from("cache/custom.ffindex"));

        let written = serde_json::to_string(&input).expect("a native input serializes");
        let reread: Input = serde_json::from_str(&written).expect("the written config loads");
        assert_eq!(
            video_cache_path(&reread),
            Some(PathBuf::from("cache/custom.ffindex")),
            "the cache path must not be lost when the config is saved and reloaded"
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InputFilterError {
    #[error(
        "These filters need VapourSynth; a native FFMS2 input can only convert bit depth, chroma \
         and resolution"
    )]
    WrongKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display, JsonSchema)]
pub enum VapourSynthImportMethod {
    /// [L-SMASH-Works](https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works)
    LSMASHWorks {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    /// [DGDecodeNV](https://www.rationalqm.us/dgdecnv/dgdecnv.html)
    DGDecNV {
        // plugin_path:          Option<PathBuf>,
        // cache_path:           Option<PathBuf>,
        dgindexnv_executable: Option<PathBuf>,
    },
    /// [FFmpegSource](https://github.com/ffms/ffms2)
    FFMS2 {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    /// [BestSource](https://github.com/vapoursynth/bestsource).
    BestSource {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display, JsonSchema)]
pub enum ImportMethod {
    // FFmpeg {}, // Unsupported
    FFMS2 {
        /// The video track to decode, as seen by FFMS2. `None` selects the
        /// first video track.
        index: Option<u8>,
        /// Where to read or write the `.ffindex` cache, defaulting to
        /// `{source}.ffindex` next to the source file.
        #[serde(default)]
        cache_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display, JsonSchema)]
pub enum VapourSynthScriptSource {
    Path(PathBuf),
    Text(String),
}
