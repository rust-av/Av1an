use std::{
    fmt::Write,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf, absolute},
    process::{Command, ExitStatus, Stdio},
    thread,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::{
    core::encoder::StringOrBytes,
    core::input::IndexProgress,
    vapoursynth::{
        VapourSynthError,
        plugins::{Plugin, PluginFunction},
        script_builder::{
            NodeVariableName, VapourSynthPluginScript,
            script::{Imports, Line, python_string_literal},
        },
    },
};

/// Original process diagnostics, including non-UTF8 console bytes. A missing
/// status means waiting failed; the OS error remains in the anyhow chain.
#[derive(Debug, thiserror::Error)]
#[error("DGIndexNV status {status:?}\nstdout: {stdout}\nstderr: {stderr}")]
pub struct DGIndexFailure {
    pub status: Option<ExitStatus>,
    pub stdout: StringOrBytes,
    pub stderr: StringOrBytes,
}

/// ponytail: reject obvious incomplete projects; leave proprietary frame tables
/// to the native decoder rather than inventing a full format parser.
/// The pinned consumer checks DGIndexNV in the first line and reads CODED/ORDER
/// in the final ten lines; the vendor HEVC sample confirms both footer records.
/// No codec/version restriction or filename decoding; only 4KiB head + 8KiB tail.
/// https://github.com/Jaded-Encoding-Thaumaturgy/vs-source/blob/ec851aa98e019a8dc6eae3a3aa6bc9e43866540b/vssource/indexers/DGIndexNV.py
/// https://github.com/Jaded-Encoding-Thaumaturgy/vs-source/blob/ec851aa98e019a8dc6eae3a3aa6bc9e43866540b/vssource/dataclasses.py
/// https://forum.doom9.org/showthread.php?t=175328
fn validate_index(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "index must be a nonempty regular file (not a symlink)"
    );
    let mut file = std::fs::File::open(path)?;
    let mut head = Vec::new();
    file.by_ref().take(4096).read_to_end(&mut head)?;
    let header = head.split(|&byte| byte == b'\n').next().unwrap_or_default();
    ensure!(
        header.windows(b"DGIndexNV".len()).any(|word| word == b"DGIndexNV"),
        "missing DGIndexNV header"
    );
    let start = metadata.len().saturating_sub(8192);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(8192).read_to_end(&mut tail)?;
    let tail = if start > 0 {
        tail.iter()
            .position(|&byte| byte == b'\n')
            .map_or(&[][..], |index| &tail[index + 1..])
    } else {
        &tail
    };
    let (mut coded, mut order) = (false, false);
    for line in tail.rsplit(|&byte| byte == b'\n').take(10) {
        let mut fields = line.split(u8::is_ascii_whitespace).filter(|field| !field.is_empty());
        let key = fields.next();
        let value = fields.next().and_then(|value| std::str::from_utf8(value).ok());
        if fields.next().is_some() {
            continue;
        }
        match (key, value) {
            (Some(b"ORDER"), Some(value)) => order = value.parse::<i64>().is_ok(),
            (Some(b"CODED"), Some(value)) => {
                coded = value.parse::<u64>().is_ok_and(|frames| frames > 0);
            },
            _ => {},
        }
        if coded && order {
            return Ok(());
        }
    }
    bail!("missing complete CODED/ORDER footer")
}

fn existing_index(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            validate_index(path).with_context(|| format!("Invalid existing DGIndexNV index {}; preserved unchanged. Check the chosen input/index path and select a valid project or input", path.display()))?;
            Ok(true)
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Opens a DGIndexNV `.dgi` project as `source`, not the original media file.
///
/// This portable source-only contract follows the [pinned upstream consumer],
/// not a verified signature for every DGDecNV release. `indexing_path` remains
/// a compatibility alias for the project path; no cache keyword is emitted.
///
/// Index generation uses a private directory beside the destination. Successful
/// exit and structural validation precede atomic, no-clobber publication; errors
/// or force before commit leave no new reusable index. Committed indices survive
/// later force. Existing invalid files are preserved: check the chosen input/index
/// path and select a valid project or input. Existing `.dgi` inputs are validated
/// directly, regardless of the cache alias.
///
/// Validation checks a regular/nonempty file, `DGIndexNV` in its first line and
/// numeric `CODED` (positive) / `ORDER` footer records in either order in its final ten lines.
/// It does not verify frame tables, media freshness, or every proprietary version.
/// Filenames need not be UTF-8; only bounded header/footer windows are read.
///
/// # Native acceptance (manual)
///
/// On a host with the licensed DGDecNV plugin and supported NVIDIA decoding,
/// set `SOURCE` to a supported video and `INDEX` to a writable `.dgi` path:
///
/// ```sh
/// dgindexnv -h -i "$SOURCE" -o "$INDEX"
/// python3 -c 'import sys, vapoursynth as vs; print([(f.name, f.signature) for f in vs.core.dgdecodenv.functions() if f.name == "DGSource"]); c = vs.core.dgdecodenv.DGSource(source=sys.argv[1]); c.get_frame(0); c.get_frame(c.num_frames - 1); print(c.width, c.height, c.num_frames)' "$INDEX"
/// ```
///
/// Record the installed signature and require both frame reads to succeed.
/// Also generate using an absolute media path into a private sibling directory,
/// move only the `.dgi` to `INDEX`, and repeat both frame reads. The cited sample
/// and pinned consumer use media paths in the project, not a required sidecar;
/// native testing must confirm relocation for the installed indexer version.
/// CPU argument-planning tests do not replace this native gate.
///
/// [pinned upstream consumer]: https://github.com/rust-av/Av1an/blob/def0a97eca67a4635fcc7e42d4c1a48381b8cd5b/av1an-core/src/loadscript.vpy#L38-L40
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DGSource {
    pub source: PathBuf,
    pub i420: Option<i32>,
    pub deinterlace: Option<i32>,
    pub use_top_field: Option<i32>,
    pub use_pf: Option<i32>,
    pub strict_avc: Option<i32>,
    pub ct: Option<i32>,
    pub cb: Option<i32>,
    pub cl: Option<i32>,
    pub cr: Option<i32>,
    pub rw: Option<i32>,
    pub rh: Option<i32>,
    pub fieldop: Option<i32>,
    pub show: Option<i32>,
    pub show2: Option<String>,
    /// Compatibility alias for the `.dgi` source path, overriding `source`.
    /// This is resolved here, not passed as an unverified native plugin option.
    pub indexing_path: Option<PathBuf>,
    pub h2s_enable: Option<i32>,
    pub h2s_white: Option<i32>,
    pub h2s_black: Option<i32>,
    pub h2s_gamma: Option<f64>,
    pub h2s_hue: Option<f64>,
    pub h2s_r: Option<f64>,
    pub h2s_g: Option<f64>,
    pub h2s_b: Option<f64>,
    pub h2s_tm: Option<f64>,
    pub h2s_roll: Option<f64>,
    pub h2s_mode: Option<String>,
    pub dn_enable: Option<i32>,
    pub dn_strength: Option<f64>,
    pub dn_cstrength: Option<f64>,
    pub dn_quality: Option<String>,
    pub dn_tthresh: Option<f64>,
    pub dn_show: Option<i32>,
    pub sh_enable: Option<i32>,
    pub sh_strength: Option<f64>,
    pub sh_os: Option<i32>,
    pub sh_us: Option<i32>,
    pub sh_top: Option<i32>,
    pub sh_bottom: Option<i32>,
    pub sh_left: Option<i32>,
    pub sh_right: Option<i32>,
    pub tw_enable: Option<i32>,
    pub tw_hue: Option<f64>,
    pub tw_sat: Option<f64>,
    pub tw_bright: Option<f64>,
    pub tw_cont: Option<f64>,
    pub tw_coring: Option<i32>,
    pub tw_start_hue: Option<i32>,
    pub tw_end_hue: Option<i32>,
    pub tw_max_sat: Option<i32>,
    pub tw_min_sat: Option<i32>,
    pub tw_interp: Option<i32>,
    pub lv_enable: Option<i32>,
    pub lv_input_low: Option<f64>,
    pub lv_gamma: Option<f64>,
    pub lv_input_high: Option<f64>,
    pub lv_output_low: Option<f64>,
    pub lv_output_high: Option<f64>,
    pub lv_chroma: Option<i32>,
    pub lv_coring: Option<i32>,
    pub lv_dither: Option<i32>,
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
        let absolute_source_path = absolute(self.indexing_path.as_ref().unwrap_or(&self.source))
            .map_err(|_| VapourSynthError::PluginFunctionError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message: "Failed to get absolute path".to_owned(),
            })?;

        Self::arguments_set(
            &mut arguments,
            vec![
                (
                    "source",
                    Some(absolute_source_path.display().to_string().as_bytes()),
                ),
                ("show2", self.show2.map(|s| s.into_bytes()).as_deref()),
                ("h2s_mode", self.h2s_mode.map(|s| s.into_bytes()).as_deref()),
                (
                    "dn_quality",
                    self.dn_quality.map(|s| s.into_bytes()).as_deref(),
                ),
            ],
        )?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
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
            ],
        )?;
        Self::arguments_set_floats(
            &mut arguments,
            vec![
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
            ],
        )?;

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }

    /// Resolve or create the `.dgi`, with optional progress as `(current, total)`.
    /// Owns the indexer's process tree through reader completion and reap;
    /// force interrupts an already admitted index without publishing it.
    #[inline]
    pub fn index_video(
        source: &Path,
        cache: Option<&Path>,
        executable: Option<&Path>,
        progress: Option<IndexProgress<'_>>,
        cancellation: &crate::utils::cancellation::Cancellation,
    ) -> Result<PathBuf> {
        cancellation.check_admission()?;
        let absolute_source = absolute(source)?;
        let absolute_cache = if source
            .extension()
            .is_some_and(|ext| ext.as_encoded_bytes().eq_ignore_ascii_case(b"dgi"))
        {
            absolute_source.clone()
        } else if let Some(c_path) = cache {
            absolute(c_path)?
        } else {
            absolute_source.with_extension("dgi")
        };
        let executable = executable.unwrap_or_else(|| Path::new("dgindexnv"));
        run_index(
            &absolute_source,
            &absolute_cache,
            executable,
            progress,
            cancellation,
        )
        .with_context(|| {
            format!(
                "DGIndexNV source {} index {} executable {}",
                absolute_source.display(),
                absolute_cache.display(),
                executable.display()
            )
        })
    }
}

fn run_index(
    source: &Path,
    cache: &Path,
    executable: &Path,
    mut progress: Option<IndexProgress<'_>>,
    cancellation: &crate::utils::cancellation::Cancellation,
) -> Result<PathBuf> {
    if existing_index(cache)? {
        cancellation.check_forced()?;
        return Ok(cache.to_owned());
    }
    ensure!(
        source != cache,
        "DGI source is missing; choose an existing .dgi project"
    );
    // Absolute -i keeps media references independent of this private directory.
    // Own potential sidecars too; never clean up a caller's directory.
    let directory = tempfile::Builder::new()
        .prefix(".av1an-dgi-")
        .tempdir_in(cache.parent().context("index has no parent")?)?;
    let candidate = tempfile::TempPath::try_from_path(directory.path().join("index.dgi"))?;
    if let Some(progress) = progress.as_mut() {
        (**progress)(0, 0);
    }
    let mut child = crate::utils::process::OwnedProcess::spawn(
        Command::new(executable)
            .arg("-h")
            .arg("-i")
            .arg(source)
            .arg("-o")
            .arg(&candidate)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        cancellation,
    )?;

    // Take both handles before starting a worker; no early return can detach it.
    let mut stderr = child.stderr.take().context("Missing DGIndexNV stderr pipe")?;
    let stdout = child.stdout.take().context("Missing DGIndexNV stdout pipe")?;
    let stderr_thread =
        thread::Builder::new().name("dgindexnv-stderr".into()).spawn(move || {
            let mut bytes = Vec::new();
            let result =
                stderr.read_to_end(&mut bytes).map(|_| ()).context("Reading DGIndexNV stderr");
            #[cfg(all(test, target_os = "linux"))]
            crate::core::input::indexing::after_stderr_read();
            Ok((bytes, result))
        })?;
    let mut reader = BufReader::new(stdout);
    let mut stdout = Vec::new();
    let read = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line);
            stdout.extend_from_slice(&line); // Capture even a failed partial read.
            #[cfg(all(test, target_os = "linux"))]
            crate::core::input::indexing::after_stdout_read()?;
            if read.context("Reading DGIndexNV stdout")? == 0 {
                return Ok(());
            }
            for chunk in String::from_utf8_lossy(&line).split(['\n', '\r']) {
                if let (Some(progress), Some((current, total))) =
                    (progress.as_mut(), parse_progress(chunk))
                {
                    (**progress)(current, total);
                }
            }
        }
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("DGIndexNV progress observer panicked")));
    let killed = if read.is_err() { child.kill() } else { Ok(()) };
    // Even on read/observer failure, join and reap before inspecting any result.
    let waited = child.wait();
    let (stderr, stderr_result) = match crate::core::encoder::pipes::join_worker(
        stderr_thread,
        "Joining DGIndexNV stderr drain",
    ) {
        Ok(captured) => captured,
        Err(error) => (Vec::new(), Err(error)),
    };
    let diagnostic = DGIndexFailure {
        status: waited.as_ref().ok().copied(),
        stdout: stdout.into(),
        stderr: stderr.into(),
    };
    (|| -> Result<()> {
        cancellation.check_forced()?;
        killed?;
        let status = waited?;
        stderr_result?;
        read?;
        ensure!(
            status.success(),
            "DGIndexNV exited unsuccessfully: {status}"
        );
        validate_index(&candidate).context("Invalid DGIndexNV candidate")
    })()
    .context(diagnostic)?;

    #[cfg(all(test, target_os = "linux"))]
    {
        crate::core::encoder::process_lifecycle::before_output_commit(cache, cancellation);
        crate::core::input::indexing::before_publish(cache, cancellation);
    }
    // persist_noclobber uses rename(NOREPLACE), or an atomic hard-link on Unix.
    // The old private link may remain on some platforms; the directory owns it.
    let published = cancellation.publish(|| match candidate.persist_noclobber(cache) {
        Ok(()) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    })?;
    if !published {
        ensure!(
            existing_index(cache)?,
            "Concurrent DGIndexNV result disappeared; retry indexing"
        );
        cancellation.check_forced()?;
    }
    #[cfg(all(test, target_os = "linux"))]
    {
        crate::core::encoder::process_lifecycle::output_drop(cache, cancellation);
        crate::core::input::indexing::after_publish(cancellation);
    }
    Ok(cache.to_owned())
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
                "core.dgdecodenv.DGSource(source = {}",
                python_string_literal(
                    &self.indexing_path.as_ref().unwrap_or(&self.source).to_string_lossy()
                )
            )?;
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
    fn dgindex_bare_percentage_is_scaled_and_clamped_to_ten_thousand() {
        assert_eq!(parse_progress("100"), Some((10_000, 10_000)));
        assert_eq!(parse_progress("45"), Some((4500, 10_000)));
        assert_eq!(parse_progress(" 100 \n"), Some((10_000, 10_000)));
        // Out-of-range values are clamped rather than sent on.
        assert_eq!(parse_progress("110"), Some((10_000, 10_000)));
    }

    /// A percentage with a sign still reads, scaled onto a 0..=10000 basis so
    /// callers compute the same value out of either form.
    #[test]
    fn dgindex_percent_suffixed_decimal_is_scaled_to_ten_thousand() {
        assert_eq!(parse_progress("45.2%"), Some((4520, 10_000)));
    }

    /// A `current/total` pair is read as frames when one is present.
    #[test]
    fn dgindex_frame_pair_progress_uses_the_last_record() {
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
