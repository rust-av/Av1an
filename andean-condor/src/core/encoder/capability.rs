use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{LazyLock, RwLock},
};

use crate::models::encoder::EncoderBase;
use crate::{core::process::OwnedProcess, utils::cancellation::Cancellation};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Capabilities that need to be checked per encoder binary
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderCapability {
    /// SVT-AV1 support for fractional/quarter-step CRF values (e.g., `--crf
    /// 25.25`)
    SvtAv1QuarterStepCrf,
    /// Encoder-native zoning: encoding a whole pass with one process while
    /// overriding the quantizer per frame range. Stock SVT-AV1 lacks `--zones`,
    /// so support depends on the installed binary.
    Zoning,
}

/// The container a raw encoder writes, which Av1an has to split per scene.
///
/// SVT-AV1 picks this from how the binary was built: with WebM IO enabled it
/// writes Matroska even to a `.ivf` path, so the file extension cannot be
/// trusted and the container is detected from the encoded bytes instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// IVF: a 32-byte file header followed by size-prefixed frames.
    Ivf,
    /// Annex-B: start-code delimited NAL units.
    AnnexB,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BinaryKey {
    encoder_base: EncoderBase,
    executable: PathBuf,
}

/// Thread-safe global cache mapping `(BinaryKey, EncoderCapability)` to a
/// support flag.
static CAPABILITY_CACHE: LazyLock<RwLock<HashMap<(BinaryKey, EncoderCapability), bool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Thread-safe global cache mapping a binary to the container it writes.
static CONTAINER_CACHE: LazyLock<RwLock<HashMap<BinaryKey, Container>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Generate a single Y4M frame stream (420, `width`x`height`) into `writer`
/// for testing encoder capabilities. `bit_depth` 8 yields `C420`, 10 yields
/// `C420p10`.
fn generate_y4m_stream<W: Write>(
    mut writer: W,
    width: u32,
    height: u32,
    bit_depth: u8,
) -> std::io::Result<()> {
    let chroma = if bit_depth == 8 {
        "C420".to_owned()
    } else {
        format!("C420p{bit_depth}")
    };
    writeln!(
        writer,
        "YUV4MPEG2 W{width} H{height} F30:1 Ip A0:0 {chroma}"
    )?;
    writeln!(writer, "FRAME")?;
    let bytes_per_sample = usize::from(bit_depth > 8) + 1;
    let y_size = (width as usize) * (height as usize) * bytes_per_sample;
    let uv_size = ((width / 2) as usize) * ((height / 2) as usize) * bytes_per_sample;
    let frame = vec![0u8; y_size + 2 * uv_size];
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// Probe encoder binary for the given capability
///
/// Returns `false` on any failure
fn probe_capability(
    encoder_base: EncoderBase,
    executable: &Path,
    capability: EncoderCapability,
    cancellation: &Cancellation,
) -> bool {
    match capability {
        EncoderCapability::SvtAv1QuarterStepCrf => {
            if encoder_base != EncoderBase::SVTAV1 {
                return false;
            }
            svt_av1_supports_quarter_step_crf(executable, cancellation)
        },
        EncoderCapability::Zoning => probe_zoning(encoder_base, executable, cancellation),
    }
}

fn svt_av1_supports_quarter_step_crf(executable: &Path, cancellation: &Cancellation) -> bool {
    let Ok(mut child) = OwnedProcess::spawn(
        Command::new(executable)
            .args([
                "-i",
                "stdin",
                "--crf",
                "50.75",
                "-b",
                crate::core::encoder::NULL_OUTPUT,
                "--progress",
                "0",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        cancellation,
    ) else {
        return false;
    };

    if let Some(stdin) = child.stdin.take() {
        // Writing may fail if the encoder rejects arguments before reading
        // input; that's fine — we only care about the exit status.
        let _ = generate_y4m_stream(stdin, 320, 240, 10);
    }

    child.wait().is_ok_and(|status| status.success())
}

/// Probe whether the encoder binary accepts zone (per-frame-range quantizer)
/// arguments together with a forced keyframe at a zone boundary.
///
/// Returns `false` on any failure.
fn probe_zoning(encoder_base: EncoderBase, executable: &Path, cancellation: &Cancellation) -> bool {
    // Declare scratch before the process owner so it survives even an early-return Drop/reap.
    let (args, bit_depth, _zone_dir) = match encoder_base {
        EncoderBase::X264 => (
            vec![
                "-".to_owned(),
                "--demuxer".to_owned(),
                "y4m".to_owned(),
                "--preset".to_owned(),
                "ultrafast".to_owned(),
                "--crf".to_owned(),
                "30".to_owned(),
                "--keyint".to_owned(),
                "infinite".to_owned(),
                "--scenecut".to_owned(),
                "0".to_owned(),
                "--log-level".to_owned(),
                "error".to_owned(),
                "--zones".to_owned(),
                "0,0,crf=30".to_owned(),
                "-o".to_owned(),
                crate::core::encoder::NULL_OUTPUT.to_owned(),
            ],
            8,
            None,
        ),
        EncoderBase::X265 => {
            let mut scratch = tempfile::Builder::new();
            scratch.prefix("av1an-zone-probe-");
            #[cfg(unix)]
            scratch.permissions(std::fs::Permissions::from_mode(0o700));
            let Ok(dir) = scratch.tempdir() else {
                return false;
            };
            let path = dir.path().join("probe.zonefile");
            if std::fs::write(&path, "0 --crf 30\n").is_err() {
                return false;
            }
            (
                vec![
                    "-".to_owned(),
                    "--y4m".to_owned(),
                    "--preset".to_owned(),
                    "ultrafast".to_owned(),
                    "--crf".to_owned(),
                    "30".to_owned(),
                    "--keyint".to_owned(),
                    "-1".to_owned(),
                    "--scenecut".to_owned(),
                    "0".to_owned(),
                    "--zonefile".to_owned(),
                    path.display().to_string(),
                    "-o".to_owned(),
                    crate::core::encoder::NULL_OUTPUT.to_owned(),
                ],
                8,
                Some(dir),
            )
        },
        EncoderBase::SVTAV1 => {
            let mut scratch = tempfile::Builder::new();
            scratch.prefix("av1an-zone-probe-");
            #[cfg(unix)]
            scratch.permissions(std::fs::Permissions::from_mode(0o700));
            let Ok(dir) = scratch.tempdir() else {
                return false;
            };
            let path = dir.path().join("probe.config");
            // Config files are keyed on the option's descriptive name, and the
            // colon separates name from value.
            if std::fs::write(&path, "Zones : 0,0,30\nForceKeyFrames : 0f\n").is_err() {
                return false;
            }
            (
                vec![
                    "-i".to_owned(),
                    "stdin".to_owned(),
                    "-b".to_owned(),
                    crate::core::encoder::NULL_OUTPUT.to_owned(),
                    "--preset".to_owned(),
                    "10".to_owned(),
                    "--crf".to_owned(),
                    "30".to_owned(),
                    "--keyint".to_owned(),
                    "0".to_owned(),
                    "--scd".to_owned(),
                    "0".to_owned(),
                    "--rc".to_owned(),
                    "0".to_owned(),
                    "--progress".to_owned(),
                    "0".to_owned(),
                    // A WebM IO build defaults to Matroska; the zone split
                    // needs IVF.
                    "--webm".to_owned(),
                    "0".to_owned(),
                    "-c".to_owned(),
                    path.display().to_string(),
                ],
                10,
                Some(dir),
            )
        },
        _ => return false,
    };

    let spawned = OwnedProcess::spawn(
        Command::new(executable)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        cancellation,
    );
    let Ok(mut child) = spawned else {
        return false;
    };

    if let Some(stdin) = child.stdin.take() {
        let _ = generate_y4m_stream(stdin, 320, 240, bit_depth);
    }

    child.wait().is_ok_and(|status| status.success())
}

/// Reads the container from the encoded bytes.
///
/// SVT-AV1 built with WebM IO writes Matroska whatever the output path says,
/// so Av1an's `.ivf` scene files can be either container; the bytes decide.
#[inline]
pub(crate) fn detect_container(bytes: &[u8]) -> Option<Container> {
    if bytes.starts_with(b"DKIF") {
        Some(Container::Ivf)
    } else if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML header: Matroska/WebM, which per-scene splitting cannot cut.
        None
    } else if bytes.starts_with(&[0x00, 0x00, 0x01]) || bytes.starts_with(&[0x00, 0x00, 0x00, 0x01])
    {
        Some(Container::AnnexB)
    } else {
        None
    }
}

pub(crate) fn encoder_container(
    encoder_base: EncoderBase,
    executable: &Path,
    cancellation: &Cancellation,
) -> anyhow::Result<Option<Container>> {
    cancellation.check_forced()?;
    let key = BinaryKey {
        encoder_base,
        executable: executable.to_path_buf(),
    };
    if let Ok(cache) = CONTAINER_CACHE.read()
        && let Some(container) = cache.get(&key)
    {
        return Ok(Some(*container));
    }

    let container = match encoder_base {
        EncoderBase::X264 | EncoderBase::X265 => Some(Container::AnnexB),
        // Probed from real output below.
        _ => None,
    }
    .or_else(|| probe_container(&key, cancellation));
    cancellation.check_forced()?;

    if let (Ok(mut cache), Some(container)) = (CONTAINER_CACHE.write(), container) {
        cache.insert(key, container);
    }
    Ok(container)
}

/// Whether the SVT-AV1 binary was built with WebM IO, which makes it write
/// Matroska unless `--webm 0` says otherwise.
///
/// `--webm` is absent from a stock build, so the option is probed by name
/// rather than assumed.
fn supports_webm_io(executable: &Path, cancellation: &Cancellation) -> bool {
    static CACHE: LazyLock<RwLock<HashMap<PathBuf, bool>>> =
        LazyLock::new(|| RwLock::new(HashMap::new()));
    if let Ok(cache) = CACHE.read()
        && let Some(&supported) = cache.get(executable)
    {
        return supported;
    }
    let supported = OwnedProcess::spawn(
        Command::new(executable)
            .arg("--full-help")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
        cancellation,
    )
    .is_ok_and(|mut child| {
        let mut output = Vec::new();
        let read = std::io::Read::read_to_end(
            &mut child.stdout.take().expect("piped help output"),
            &mut output,
        );
        let waited = child.wait();
        read.is_ok() && waited.is_ok() && String::from_utf8_lossy(&output).contains("--webm")
    });
    if !cancellation.is_forced()
        && let Ok(mut cache) = CACHE.write()
    {
        cache.insert(executable.to_path_buf(), supported);
    }
    supported
}

/// Encodes one throwaway frame and inspects the container it produced.
fn probe_container(key: &BinaryKey, cancellation: &Cancellation) -> Option<Container> {
    if key.encoder_base != EncoderBase::SVTAV1 {
        return None;
    }
    // Outlive help/encode owners, including Drop on errors, and the output read below.
    let mut scratch = tempfile::Builder::new();
    scratch.prefix("av1an-container-probe-");
    #[cfg(unix)]
    scratch.permissions(std::fs::Permissions::from_mode(0o700));
    let dir = scratch.tempdir().ok()?;
    let path = dir.path().join("probe.out");
    let mut args = vec![
        "-i".to_owned(),
        "stdin".to_owned(),
        "-b".to_owned(),
        path.display().to_string(),
        "--preset".to_owned(),
        "12".to_owned(),
        "--progress".to_owned(),
        "0".to_owned(),
        "--crf".to_owned(),
        "32".to_owned(),
    ];
    // A WebM IO build writes Matroska by default; Zone Encoder passes this to
    // force the IVF it can split, so the probe must pass it too or it reports
    // a different container than the encode produces.
    if supports_webm_io(&key.executable, cancellation) {
        args.extend(["--webm".to_owned(), "0".to_owned()]);
    }
    let Ok(mut child) = OwnedProcess::spawn(
        Command::new(&key.executable)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        cancellation,
    ) else {
        return None;
    };
    if let Some(stdin) = child.stdin.take() {
        let _ = generate_y4m_stream(stdin, 320, 240, 10);
    }
    if !child.wait().is_ok_and(|status| status.success()) {
        return None;
    }
    std::fs::read(&path).ok().as_deref().and_then(detect_container)
}

pub(crate) fn check_capability(
    encoder_base: EncoderBase,
    executable: &Path,
    capability: EncoderCapability,
    cancellation: &Cancellation,
) -> anyhow::Result<bool> {
    cancellation.check_forced()?;
    let key = BinaryKey {
        encoder_base,
        executable: executable.to_path_buf(),
    };

    // Read result from cache
    if let Ok(cache) = CAPABILITY_CACHE.read()
        && let Some(&supported) = cache.get(&(key.clone(), capability))
    {
        return Ok(supported);
    }

    // Write result to cache
    let supported = probe_capability(encoder_base, &key.executable, capability, cancellation);
    cancellation.check_forced()?;

    if let Ok(mut cache) = CAPABILITY_CACHE.write() {
        cache.insert((key, capability), supported);
    }

    Ok(supported)
}

#[cfg(test)]
#[cfg(unix)]
#[path = "capability/tests.rs"]
mod tests;
