use std::{
    fmt::Write,
    io::{BufRead, BufReader},
    path::{Path, PathBuf, absolute},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::{
    core::input::IndexProgress,
    vapoursynth::{
        VapourSynthError,
        plugins::{Plugin, PluginFunction},
        script_builder::{
            NodeVariableName,
            VapourSynthPluginScript,
            script::{Imports, Line},
        },
    },
};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DGSource {
    pub source:         PathBuf,
    pub i420:           Option<i32>,
    pub deinterlace:    Option<i32>,
    pub use_top_field:  Option<i32>,
    pub use_pf:         Option<i32>,
    pub strict_avc:     Option<i32>,
    pub ct:             Option<i32>,
    pub cb:             Option<i32>,
    pub cl:             Option<i32>,
    pub cr:             Option<i32>,
    pub rw:             Option<i32>,
    pub rh:             Option<i32>,
    pub fieldop:        Option<i32>,
    pub show:           Option<i32>,
    pub show2:          Option<String>,
    pub indexing_path:  Option<PathBuf>,
    pub h2s_enable:     Option<i32>,
    pub h2s_white:      Option<i32>,
    pub h2s_black:      Option<i32>,
    pub h2s_gamma:      Option<f64>,
    pub h2s_hue:        Option<f64>,
    pub h2s_r:          Option<f64>,
    pub h2s_g:          Option<f64>,
    pub h2s_b:          Option<f64>,
    pub h2s_tm:         Option<f64>,
    pub h2s_roll:       Option<f64>,
    pub h2s_mode:       Option<String>,
    pub dn_enable:      Option<i32>,
    pub dn_strength:    Option<f64>,
    pub dn_cstrength:   Option<f64>,
    pub dn_quality:     Option<String>,
    pub dn_tthresh:     Option<f64>,
    pub dn_show:        Option<i32>,
    pub sh_enable:      Option<i32>,
    pub sh_strength:    Option<f64>,
    pub sh_os:          Option<i32>,
    pub sh_us:          Option<i32>,
    pub sh_top:         Option<i32>,
    pub sh_bottom:      Option<i32>,
    pub sh_left:        Option<i32>,
    pub sh_right:       Option<i32>,
    pub tw_enable:      Option<i32>,
    pub tw_hue:         Option<f64>,
    pub tw_sat:         Option<f64>,
    pub tw_bright:      Option<f64>,
    pub tw_cont:        Option<f64>,
    pub tw_coring:      Option<i32>,
    pub tw_start_hue:   Option<i32>,
    pub tw_end_hue:     Option<i32>,
    pub tw_max_sat:     Option<i32>,
    pub tw_min_sat:     Option<i32>,
    pub tw_interp:      Option<i32>,
    pub lv_enable:      Option<i32>,
    pub lv_input_low:   Option<f64>,
    pub lv_gamma:       Option<f64>,
    pub lv_input_high:  Option<f64>,
    pub lv_output_low:  Option<f64>,
    pub lv_output_high: Option<f64>,
    pub lv_chroma:      Option<i32>,
    pub lv_coring:      Option<i32>,
    pub lv_dither:      Option<i32>,
}

impl Plugin for DGSource {
    const PLUGIN_NAME: &'static str = "DGDecNV";
    const PLUGIN_ID: &'static str = "com.vapoursynth.dgdecodenv";
    const PLUGIN_DOCS: Option<&'static str> =
        Some("https://www.rationalqm.us/dgdecnv/dgdecnv.html");
}

impl PluginFunction for DGSource {
    const FUNCTION_NAME: &'static str = "DGSource";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.rationalqm.us/dgdecnv/QuickStart.html");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("source", &ValueType::Data)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("i420", &ValueType::Int),
        ("i422", &ValueType::Int),
        ("i444", &ValueType::Int),
        ("i420", &ValueType::Int),
        ("deinterlace", &ValueType::Int),
        ("use_top_field", &ValueType::Int),
        ("use_pf", &ValueType::Int),
        ("strict_avc", &ValueType::Int),
        ("ct", &ValueType::Int),
        ("cb", &ValueType::Int),
        ("cl", &ValueType::Int),
        ("cr", &ValueType::Int),
        ("rw", &ValueType::Int),
        ("rh", &ValueType::Int),
        ("fieldop", &ValueType::Int),
        ("show", &ValueType::Int),
        ("h2s_enable", &ValueType::Int),
        ("h2s_white", &ValueType::Int),
        ("h2s_black", &ValueType::Int),
        ("h2s_gamma", &ValueType::Int),
        ("dn_enable", &ValueType::Int),
        ("dn_show", &ValueType::Int),
        ("sh_enable", &ValueType::Int),
        ("sh_os", &ValueType::Int),
        ("sh_us", &ValueType::Int),
        ("sh_top", &ValueType::Int),
        ("sh_bottom", &ValueType::Int),
        ("sh_left", &ValueType::Int),
        ("sh_right", &ValueType::Int),
        ("tw_enable", &ValueType::Int),
        ("tw_coring", &ValueType::Int),
        ("tw_startHue", &ValueType::Int),
        ("tw_endHue", &ValueType::Int),
        ("tw_maxSat", &ValueType::Int),
        ("tw_minSat", &ValueType::Int),
        ("tw_interp", &ValueType::Int),
        ("lv_enable", &ValueType::Int),
        ("lv_chroma", &ValueType::Int),
        ("lv_coring", &ValueType::Int),
        ("lv_dither", &ValueType::Int),
        ("h2s_hue", &ValueType::Float),
        ("h2s_r", &ValueType::Float),
        ("h2s_g", &ValueType::Float),
        ("h2s_b", &ValueType::Float),
        ("h2s_tm", &ValueType::Float),
        ("h2s_roll", &ValueType::Float),
        ("dn_strength", &ValueType::Float),
        ("dn_cstrength", &ValueType::Float),
        ("dn_tthresh", &ValueType::Float),
        ("sh_strength", &ValueType::Float),
        ("tw_hue", &ValueType::Float),
        ("tw_sat", &ValueType::Float),
        ("tw_bright", &ValueType::Float),
        ("tw_cont", &ValueType::Float),
        ("lv_input_low", &ValueType::Float),
        ("lv_gamma", &ValueType::Float),
        ("lv_input_high", &ValueType::Float),
        ("lv_output_low", &ValueType::Float),
        ("lv_output_high", &ValueType::Float),
    ];
}

impl DGSource {
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
        let absolute_indexing_path = if let Some(indexing_path) = self.indexing_path {
            let absolute_indexing_path =
                absolute(indexing_path).map_err(|_| VapourSynthError::PluginFunctionError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    function: Self::FUNCTION_NAME.to_owned(),
                    message:  "Failed to get absolute path".to_owned(),
                })?;
            Some(absolute_indexing_path)
        } else {
            None
        };

        Self::arguments_set(&mut arguments, vec![
            (
                "source",
                Some(absolute_source_path.display().to_string().as_bytes()),
            ),
            ("show2", self.show2.map(|s| s.into_bytes()).as_deref()),
            (
                "indexing_path",
                absolute_indexing_path.map(|p| p.display().to_string().into_bytes()).as_deref(),
            ),
            ("h2s_mode", self.h2s_mode.map(|s| s.into_bytes()).as_deref()),
            (
                "dn_quality",
                self.dn_quality.map(|s| s.into_bytes()).as_deref(),
            ),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("i420", self.i420),
            ("deinterlace", self.deinterlace),
            ("use_top_field", self.use_top_field),
            ("use_pf", self.use_pf),
            ("strict_avc", self.strict_avc),
            ("ct", self.ct),
            ("cb", self.cb),
            ("cl", self.cl),
            ("cr", self.cr),
            ("rw", self.rw),
            ("rh", self.rh),
            ("fieldop", self.fieldop),
            ("show", self.show),
            ("h2s_enable", self.h2s_enable),
            ("h2s_white", self.h2s_white),
            ("h2s_black", self.h2s_black),
            ("dn_enable", self.dn_enable),
            ("dn_show", self.dn_show),
            ("sh_enable", self.sh_enable),
            ("sh_os", self.sh_os),
            ("sh_us", self.sh_us),
            ("sh_top", self.sh_top),
            ("sh_bottom", self.sh_bottom),
            ("sh_left", self.sh_left),
            ("sh_right", self.sh_right),
            ("tw_enable", self.tw_enable),
            ("tw_coring", self.tw_coring),
            ("tw_startHue", self.tw_start_hue),
            ("tw_endHue", self.tw_end_hue),
            ("tw_maxSat", self.tw_max_sat),
            ("tw_minSat", self.tw_min_sat),
            ("tw_interp", self.tw_interp),
            ("lv_enable", self.lv_enable),
            ("lv_chroma", self.lv_chroma),
            ("lv_coring", self.lv_coring),
            ("lv_dither", self.lv_dither),
        ])?;
        Self::arguments_set_floats(&mut arguments, vec![
            ("h2s_gamma", self.h2s_gamma),
            ("h2s_hue", self.h2s_hue),
            ("h2s_r", self.h2s_r),
            ("h2s_g", self.h2s_g),
            ("h2s_b", self.h2s_b),
            ("h2s_tm", self.h2s_tm),
            ("h2s_roll", self.h2s_roll),
            ("dn_strength", self.dn_strength),
            ("dn_cstrength", self.dn_cstrength),
            ("dn_tthresh", self.dn_tthresh),
            ("sh_strength", self.sh_strength),
            ("tw_hue", self.tw_hue),
            ("tw_sat", self.tw_sat),
            ("tw_bright", self.tw_bright),
            ("tw_cont", self.tw_cont),
            ("lv_input_low", self.lv_input_low),
            ("lv_gamma", self.lv_gamma),
            ("lv_input_high", self.lv_input_high),
            ("lv_output_low", self.lv_output_low),
            ("lv_output_high", self.lv_output_high),
        ])?;

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }

    /// Attempt to index a video file using `dgindexnv` or the provided
    /// executable and save the resulting cache file to the provided path or
    /// next to the source file. If the cache file already exists, indexing will
    /// be skipped.
    ///
    /// DGIndexNV runs out of process, so its console output is read line by
    /// line rather than swallowed by [`Command::output`] and forwarded to
    /// `progress` as `(current, total)`. Pass `cancelled` to kill the child
    /// part-way through.
    #[inline]
    pub fn index_video(
        source: &Path,
        cache: Option<&Path>,
        executable: Option<&Path>,
        mut progress: Option<IndexProgress<'_>>,
        cancelled: Option<&AtomicBool>,
    ) -> Result<PathBuf> {
        let absolute_source = absolute(source)?;
        let absolute_cache = if let Some(c_path) = cache {
            absolute(c_path)?
        } else {
            absolute_source.with_extension("dgi")
        };
        if absolute_cache.exists() {
            return Ok(absolute_cache);
        }
        // The cache is known missing from here on, so this is the only point
        // that can honestly claim indexing is starting.
        if let Some(progress) = progress.as_mut() {
            (**progress)(0, 0);
        }

        let mut child = Command::new(
            executable.map_or_else(|| "dgindexnv".to_owned(), |exe| exe.display().to_string()),
        )
        .arg("-h")
        .arg("-i")
        .arg(&absolute_source)
        .arg("-o")
        .arg(&absolute_cache)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| VapourSynthError::PluginFunctionError {
            plugin:   DGSource::PLUGIN_NAME.to_owned(),
            function: DGSource::FUNCTION_NAME.to_owned(),
            message:  "Failed to index video".to_owned(),
        })?;

        // Drained on its own thread so DGIndexNV can never stall writing to a
        // pipe nobody is reading. Kept for diagnostics; the exit status is
        // still ignored, exactly as it was when this used `output()`.
        let mut stderr =
            child.stderr.take().ok_or_else(|| VapourSynthError::PluginFunctionError {
                plugin:   DGSource::PLUGIN_NAME.to_owned(),
                function: DGSource::FUNCTION_NAME.to_owned(),
                message:  "Failed to read DGIndexNV stderr".to_owned(),
            })?;
        let stderr_thread = thread::spawn(move || {
            let mut captured = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut captured);
            captured
        });

        let stdout = child.stdout.take().ok_or_else(|| VapourSynthError::PluginFunctionError {
            plugin:   DGSource::PLUGIN_NAME.to_owned(),
            function: DGSource::FUNCTION_NAME.to_owned(),
            message:  "Failed to read DGIndexNV stdout".to_owned(),
        })?;
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let mut read_error = None;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                // Progress is rewritten in place with carriage returns, so both
                // terminators delimit a report and `\r\n` yields empty chunks.
                Ok(_) => {
                    for chunk in String::from_utf8_lossy(&line).split(['\n', '\r']) {
                        if let (Some(progress), Some((current, total))) =
                            (progress.as_mut(), parse_progress(chunk))
                        {
                            (**progress)(current, total);
                        }
                    }
                    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stderr_thread.join();
                        anyhow::bail!("indexing of {} was cancelled", source.display());
                    }
                },
                Err(error) => {
                    read_error = Some(error);
                    break;
                },
            }
        }
        // A failed read must still reap the child, or `dgindexnv` outlives us.
        if let Some(error) = read_error {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_thread.join();
            return Err(error.into());
        }

        let _ = child.wait()?;
        let captured = stderr_thread.join().unwrap_or_default();
        if !captured.trim().is_empty() {
            tracing::debug!("DGIndexNV stderr: {}", captured.trim());
        }

        Ok(absolute_cache)
    }
}

/// Extract `(current, total)` from one chunk of DGIndexNV output.
///
/// DGIndexNV writes the progress percentage to stdout for third-party tools,
/// one item per line under a label such as `Project`. A bare integer is the
/// common form (`100`), so it is matched against the whole line rather than a
/// token — otherwise `DGIndexNV 2.0.0.73` or a frame count would read as
/// progress. A `current/total` pair is taken as frames when present, and a
/// percentage is scaled onto a 0..=10000 basis so both reach callers in the
/// same shape.
fn parse_progress(chunk: &str) -> Option<(u64, u64)> {
    let chunk = chunk.trim();

    let mut frames: Option<(u64, u64)> = None;
    for token in chunk.split_whitespace() {
        if let Some((current, total)) = token.split_once('/')
            && let (Ok(current), Ok(total)) = (current.parse::<u64>(), total.parse::<u64>())
            && total > 0
        {
            frames = Some((current, total));
        }
    }

    let percent = chunk
        .strip_suffix('%')
        .unwrap_or(chunk)
        .parse::<f64>()
        .ok()
        .map(|value| value.clamp(0.0, 100.0));

    frames.or_else(|| percent.map(|value| ((value * 100.0).round() as u64, 10_000)))
}

impl VapourSynthPluginScript for DGSource {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.dgdecodenv.DGSource(source = r\"{}\"",
                self.source.display()
            )?;
            if let Some(cache) = &self.indexing_path {
                write!(&mut line, ", cachepath = r\"{}\"", cache.display())?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_progress;

    /// What DGIndexNV actually writes: the percentage as a bare integer on its
    /// own line, under a label. Captured from DGIndexNV indexing real MKVs.
    #[test]
    fn a_bare_integer_line_is_the_percentage() {
        assert_eq!(parse_progress("100"), Some((10_000, 10_000)));
        assert_eq!(parse_progress("45"), Some((4500, 10_000)));
        assert_eq!(parse_progress(" 100 \n"), Some((10_000, 10_000)));
        // Out-of-range values are clamped rather than sent on.
        assert_eq!(parse_progress("110"), Some((10_000, 10_000)));
    }

    /// A percentage with a sign still reads, scaled onto a 0..=10000 basis so
    /// callers compute the same value out of either form.
    #[test]
    fn a_signed_percentage_is_scaled_to_a_basis_of_ten_thousand() {
        assert_eq!(parse_progress("45.2%"), Some((4520, 10_000)));
    }

    /// A `current/total` pair is read as frames when one is present.
    #[test]
    fn a_frame_pair_is_read_as_frames() {
        assert_eq!(parse_progress("Indexing 1234/2468"), Some((1234, 2468)));
        // The last pair on a rewritten line is the newest.
        assert_eq!(
            parse_progress("Indexing 100/2468  Indexing 2400/2468"),
            Some((2400, 2468))
        );
    }

    /// Labels, versions and paths share stdout with the percentage. Matching
    /// the whole line rather than any token is what keeps `2.0.0.73` and a
    /// frame count out of the progress bar.
    #[test]
    fn non_progress_output_is_ignored() {
        assert_eq!(parse_progress("Project"), None);
        assert_eq!(parse_progress("DGIndexNV 2.0.0.73"), None);
        assert_eq!(parse_progress("D:/Videos/input.m2ts"), None);
        assert_eq!(parse_progress("Frame 100"), None);
        assert_eq!(parse_progress(""), None);
        assert_eq!(parse_progress("   "), None);
    }

    /// Zero of zero frames names no progress at all.
    #[test]
    fn a_zero_total_is_ignored() {
        assert_eq!(parse_progress("0/0"), None);
    }
}
