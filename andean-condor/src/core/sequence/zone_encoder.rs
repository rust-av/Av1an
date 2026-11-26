//! Zone encoding: one encoder process for an entire probe pass.
//!
//! [`ParallelEncoder`](super::parallel_encoder::ParallelEncoder) spawns an
//! encoder per scene, paying process startup and decode overhead for every
//! scene — expensive when scenes are only a handful of frames long. When every
//! scene differs only in its quantizer and all scenes share one encoder,
//! encoder-native zoning sets the quantizer per frame range inside a single
//! process, and a forced keyframe at each zone start breaks the bitstream
//! where an independent per-scene encode would.
//!
//! [`ZonePlan::try_build`] decides eligibility (falling back to parallel
//! encoding is always safe), [`ZoneEncoder::encode_tasks`] encodes the whole
//! pass and splits the stream back into the per-scene files.

use std::{
    collections::VecDeque,
    fmt::{self, Write as _},
    fs,
    io::{BufRead, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{self, Arc, Mutex, mpsc},
    thread,
    time::SystemTime,
};

use crate::utils::cancellation::Cancellation;
use anyhow::{Context, Result, bail};
use thiserror::Error;
use tracing::debug;

use crate::{
    core::{
        encoder::{Container, EncodeProgress, EncoderCapability, EncoderResult},
        input::Input,
        sequence::{
            SequenceCompletion, SequenceStatus, Status,
            ffmpeg_filter::FfmpegFilter,
            parallel_encoder::{ParallelEncoderError, ParallelEncoderResult, Task},
        },
    },
    models::encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
};

/// Zone and keyframe options Av1an sets itself. A scene that configures one
/// would not be reproduced by zoning, so the presence of any of them makes the
/// pass fall back to parallel encoding.
const RESERVED_OPTIONS: &[&str] =
    &["zones", "zonefile", "qpfile", "force-key-frames", "config", "webm"];

/// One scene's slice of the zone-encoded stream, in stream frame positions.
///
/// Positions count frames of the encoded pass, not source frame numbers: a
/// probe pass stores only the frames it selected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// First stream frame of the zone (inclusive).
    pub start: usize,
    /// Last stream frame of the zone (inclusive).
    pub end: usize,
    /// The scene's quantizer, applied to the whole zone.
    pub quantizer: f64,
}

/// Why a pass cannot be zone encoded. Every reason is a safe signal to fall
/// back to [`ParallelEncoder`](super::parallel_encoder::ParallelEncoder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ZoneIneligible {
    /// There is nothing to encode.
    #[error("no scenes to encode")]
    Empty,
    /// A scene contributes no frames.
    #[error("scene {scene} has no frames")]
    EmptyTask { scene: usize },
    /// Av1an has no zoning implementation for this encoder.
    #[error("encoder {0} does not support zoning")]
    UnsupportedEncoder(EncoderBase),
    /// Zone options apply to a single-pass encode only.
    #[error("scene {scene} is not a single-pass encode")]
    NotSinglePass { scene: usize },
    /// The scene configures a zone or keyframe option itself.
    #[error("scene {scene} already configures a zone or keyframe option")]
    ReservedOption { scene: usize },
    /// Scenes differ beyond their quantizer, or use different encoders.
    #[error("scene {scene} differs from the first scene beyond the quantizer: {field}")]
    DifferingEncoder {
        scene: usize,
        /// The first encoder field found to differ.
        field: DifferingField,
    },
    /// The scene's encoder has no quantizer to zone by.
    #[error("scene {scene} has no quantizer")]
    NoQuantizer { scene: usize },
    /// The installed binary rejected zone arguments in a capability probe.
    #[error("the encoder binary does not support zone encoding")]
    NoZoneSupport,
    /// The encoder writes a container that cannot be split back into scenes.
    #[error("the encoder writes a container that cannot be split per scene")]
    UnsplittableContainer,
}

/// Which part of a scene encoder differs from the first scene's, quantizer
/// aside.
///
/// Carried by [`ZoneIneligible::DifferingEncoder`] so the fallback log names
/// the field to look at instead of reporting a generic mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferingField {
    /// The encoder base (x264, x265, SVT-AV1, ...).
    Base,
    /// The encoder binary.
    Executable,
    /// The pass configuration.
    Passes,
    /// SVT-AV1 photon noise.
    PhotonNoise,
    /// The FFmpeg filtergraph.
    FfmpegFilter,
    /// Whether colour metadata is appended.
    AppendColorData,
    /// The encoder options once the quantizer is normalized away.
    Options,
}

impl fmt::Display for DifferingField {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Base => "encoder base",
            Self::Executable => "executable",
            Self::Passes => "passes",
            Self::PhotonNoise => "photon_noise",
            Self::FfmpegFilter => "ffmpeg_filter",
            Self::AppendColorData => "append_color_data",
            Self::Options => "options",
        })
    }
}

/// The zoning layout of a pass: contiguous stream ranges, one per scene, each
/// carrying that scene's quantizer, plus the encoder arguments derived from
/// them.
#[derive(Debug, Clone, PartialEq)]
pub struct ZonePlan {
    zones: Vec<Zone>,
    /// `--zones` value (x264) or zonefile body (x265).
    zone_option: String,
    /// qpfile body forcing an IDR at every zone start past the first; empty
    /// when none are needed.
    qpfile_content: String,
    /// SVT-AV1 config file body (`Name : value` lines), which carries its
    /// zones and keyframes instead of the command line.
    config_content: String,
}

impl ZonePlan {
    /// Build a zoning plan, or return a downcastable [`ZoneIneligible`] reason.
    /// Check settings before spawning a capability probe. Cancellation or failure
    /// to resolve the configured executable is not an ineligibility reason.
    #[inline]
    pub fn try_build(tasks: &[Task], cancellation: &Cancellation) -> anyhow::Result<ZonePlan> {
        let plan = Self::settings(tasks)?;
        cancellation.check_admission()?;
        let first = &tasks[0];
        if !first.encoder.supports_capability(EncoderCapability::Zoning, cancellation)? {
            return Err(ZoneIneligible::NoZoneSupport.into());
        }
        if first.encoder.container(cancellation)?.is_none() {
            return Err(ZoneIneligible::UnsplittableContainer.into());
        }
        Ok(plan)
    }

    fn settings(tasks: &[Task]) -> Result<ZonePlan, ZoneIneligible> {
        let first = tasks.first().ok_or(ZoneIneligible::Empty)?;
        let base = first.encoder.base();
        if !matches!(
            base,
            EncoderBase::X264 | EncoderBase::X265 | EncoderBase::SVTAV1
        ) {
            return Err(ZoneIneligible::UnsupportedEncoder(base));
        }
        if !matches!(first.encoder.passes(), EncoderPasses::All(1)) {
            return Err(ZoneIneligible::NotSinglePass {
                scene: first.original_index,
            });
        }
        if RESERVED_OPTIONS.iter().any(|key| first.encoder.parameters().contains_key(*key)) {
            return Err(ZoneIneligible::ReservedOption {
                scene: first.original_index,
            });
        }
        for task in tasks {
            if task.frame_indices.is_empty() {
                return Err(ZoneIneligible::EmptyTask {
                    scene: task.original_index,
                });
            }
            if let Some(field) = differing_field(&first.encoder, &task.encoder) {
                return Err(ZoneIneligible::DifferingEncoder {
                    scene: task.original_index,
                    field,
                });
            }
        }

        let zones = Self::from_tasks(tasks)?;
        let (zone_option, qpfile_content, config_content) = format_zone_options(base, &zones);

        Ok(ZonePlan {
            zones,
            zone_option,
            qpfile_content,
            config_content,
        })
    }

    /// Contiguous stream ranges and quantizers for each scene.
    fn from_tasks(tasks: &[Task]) -> Result<Vec<Zone>, ZoneIneligible> {
        let mut zones = Vec::with_capacity(tasks.len());
        let mut start = 0usize;
        for task in tasks {
            let frames = task.frame_indices.len();
            let quantizer = task.encoder.quantizer().ok_or(ZoneIneligible::NoQuantizer {
                scene: task.original_index,
            })?;
            zones.push(Zone {
                start,
                end: start + frames - 1,
                quantizer,
            });
            start += frames;
        }
        Ok(zones)
    }
}

/// The first part of `other`'s encoder that differs from `reference`'s apart
/// from the quantizer, or `None` when the two encode the same way: same base,
/// binary, passes, FFmpeg filter, colour-data appending, photon noise and
/// options once the quantizer is normalized away.
fn differing_field(reference: &Encoder, other: &Encoder) -> Option<DifferingField> {
    if reference.base() != other.base() {
        return Some(DifferingField::Base);
    }
    if reference.executable() != other.executable() {
        return Some(DifferingField::Executable);
    }
    if !passes_equivalent(reference.passes(), other.passes()) {
        return Some(DifferingField::Passes);
    }
    if reference.ffmpeg_filter() != other.ffmpeg_filter() {
        return Some(DifferingField::FfmpegFilter);
    }
    if reference.append_color_data() != other.append_color_data() {
        return Some(DifferingField::AppendColorData);
    }
    if !photon_noise_equivalent(reference, other) {
        return Some(DifferingField::PhotonNoise);
    }
    let mut reference = reference.clone();
    let mut other = other.clone();
    reference.set_quantizer(0.0);
    other.set_quantizer(0.0);
    if reference.parameters() != other.parameters() {
        return Some(DifferingField::Options);
    }
    None
}

/// Whether two scene encoders carry identical photon noise. SVTAV1 is the only
/// zone-capable base that carries it.
#[inline]
fn photon_noise_equivalent(reference: &Encoder, other: &Encoder) -> bool {
    match (reference, other) {
        (
            Encoder::SVTAV1 {
                photon_noise: a, ..
            },
            Encoder::SVTAV1 {
                photon_noise: b, ..
            },
        ) => a == b,
        _ => true,
    }
}

#[inline]
fn passes_equivalent(reference: EncoderPasses, other: EncoderPasses) -> bool {
    match (reference, other) {
        (EncoderPasses::All(a), EncoderPasses::All(b)) => a == b,
        (EncoderPasses::Specific(a, b), EncoderPasses::Specific(c, d)) => a == c && b == d,
        _ => false,
    }
}

/// Formats the zone option values for `base`.
///
/// x264 takes inline `start,end,crf=q` ranges joined by `/`; x265 reads
/// `start --crf q` lines from a file. SVT-AV1 reads a config file of
/// `Name : value` lines, which keeps its zone and keyframe lists off the
/// command line. Keyframes: x264/x265 use a qpfile (`I` forces an IDR),
/// SVT-AV1 uses `ForceKeyFrames` with `Nf` frame specifiers. Stream frame 0 is
/// already an IDR, so it needs no entry.
///
/// Returns `(inline_zone_option, qpfile_content, config_content)`; a field
/// that the encoder does not use is empty.
fn format_zone_options(base: EncoderBase, zones: &[Zone]) -> (String, String, String) {
    let mut qpfile_content = String::new();
    for zone in zones.iter().filter(|zone| zone.start > 0) {
        let _ = writeln!(qpfile_content, "{} I", zone.start);
    }
    match base {
        EncoderBase::X264 => (
            zones
                .iter()
                .map(|zone| format!("{},{},crf={}", zone.start, zone.end, zone.quantizer))
                .collect::<Vec<_>>()
                .join("/"),
            qpfile_content,
            String::new(),
        ),
        EncoderBase::X265 => (
            zones
                .iter()
                .map(|zone| format!("{} --crf {}", zone.start, zone.quantizer))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
            qpfile_content,
            String::new(),
        ),
        EncoderBase::SVTAV1 => {
            // The config file is keyed on the option's descriptive name, not
            // its CLI token: `--zones` is rejected, `Zones` is accepted.
            let mut config = String::new();
            let _ = writeln!(
                config,
                "Zones : {}",
                zones
                    .iter()
                    .map(|zone| { format!("{},{},{}", zone.start, zone.end, zone.quantizer) })
                    .collect::<Vec<_>>()
                    .join(";")
            );
            let keyframes = zones
                .iter()
                .filter(|zone| zone.start > 0)
                .map(|zone| format!("{}f", zone.start))
                .collect::<Vec<_>>()
                .join(",");
            if !keyframes.is_empty() {
                let _ = writeln!(config, "ForceKeyFrames : {keyframes}");
            }
            (String::new(), String::new(), config)
        },
        _ => unreachable!("zoning eligibility restricts the base"),
    }
}

/// Writes the plan's zone files into `directory` and injects their options
/// into a clone of the scene encoder. Returns the files to delete afterwards.
fn apply_zone_options(
    encoder: &mut Encoder,
    plan: &ZonePlan,
    directory: &Path,
) -> Result<Vec<PathBuf>> {
    let base = encoder.base();
    let mut zone_files = Vec::new();
    let mut insert = |key: &str, value: &str| {
        encoder
            .parameters_mut()
            .insert(key.to_owned(), CLIParameter::new_string("--", " ", value));
    };
    match base {
        EncoderBase::X264 => {
            insert("zones", &plan.zone_option);
            if !plan.qpfile_content.is_empty() {
                let path = directory.join("zone.qpfile");
                fs::write(&path, &plan.qpfile_content)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                insert("qpfile", &path.display().to_string());
                zone_files.push(path);
            }
        },
        EncoderBase::X265 => {
            let path = directory.join("zone.zonefile");
            fs::write(&path, &plan.zone_option)
                .with_context(|| format!("failed to write {}", path.display()))?;
            insert("zonefile", &path.display().to_string());
            zone_files.push(path);
            if !plan.qpfile_content.is_empty() {
                let path = directory.join("zone.qpfile");
                fs::write(&path, &plan.qpfile_content)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                insert("qpfile", &path.display().to_string());
                zone_files.push(path);
            }
        },
        EncoderBase::SVTAV1 => {
            // `-c` combines with the command line, so only the zone and
            // keyframe lists move into the file. A WebM IO build writes
            // Matroska to a `.ivf` path; this keeps the container IVF so the
            // pass can be split per scene.
            let path = directory.join("zone.config");
            fs::write(&path, &plan.config_content)
                .with_context(|| format!("failed to write {}", path.display()))?;
            insert("webm", "0");
            insert("config", &path.display().to_string());
            zone_files.push(path);
        },
        _ => unreachable!("zoning eligibility restricts the base"),
    }
    Ok(zone_files)
}

pub struct ZoneEncoder;

impl ZoneEncoder {
    /// Encodes a planned zone batch in one attempt. The count check is optional;
    /// a successful exit and nonempty output are always required.
    #[inline]
    #[allow(clippy::needless_pass_by_value)]
    pub fn encode_tasks(
        input: &mut Input,
        mut tasks: VecDeque<Task>,
        plan: ZonePlan,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
        validate_frame_count: bool,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        cancelled.check_forced()?;
        if cancelled.is_cancelled() {
            return Ok(vec![None; tasks.len()]);
        }
        if tasks.is_empty() {
            return Ok(Vec::new());
        }
        let clip_info = input.clip_info()?;
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        for task in &tasks {
            super::parallel_encoder::checked_bitrate(0, task.frame_indices.len(), framerate)
                .with_context(|| format!("Scene {} zone bitrate preflight", task.original_index))?;
            task.encoder.validate()?;
        }
        super::parallel_encoder::artifact::preflight(validate_frame_count)?;
        let cancelled = cancelled.active();
        let total_tasks = tasks.len();
        let total_frames: usize = tasks.iter().map(|task| task.frame_indices.len()).sum();
        debug!("Zone encoding {total_tasks} scenes ({total_frames} frames) in one process");

        let first = tasks.front().expect("zone plan was built from tasks");
        let directory = first.output.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mut encoder = first.encoder.clone();
        let started = SystemTime::now();
        let header = Cursor::new(input.y4m_header(Some(total_frames))?.into_bytes());
        let mut reader = input.frame_reader()?;
        let zone_directory =
            tempfile::tempdir_in(&directory).context("Creating owned zone files")?;
        let zone_files = apply_zone_options(&mut encoder, &plan, zone_directory.path())?;
        let extension = encoder.base().output_extension();
        let whole_output = directory.join(format!("zone-pass.temp.{extension}"));
        let mut partial = crate::core::process::InterruptedOutput::new(&whole_output, &cancelled);
        let discard = {
            let whole_output = whole_output.clone();
            move || {
                let _ = fs::remove_file(&whole_output);
                for path in &zone_files {
                    let _ = fs::remove_file(path);
                }
            }
        };

        // Encoder progress reports whole-stream frame counts; the receiver
        // side of the pass re-labels them "Encode".
        let (encode_progress_tx, encode_progress_rx) = mpsc::channel::<EncodeProgress>();
        let relay_tx = progress_tx.clone();
        let relay = thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                if progress.pass.0 == progress.pass.1 && progress.frame > 0 {
                    relay_tx.send(SequenceStatus::Whole(Status::Processing {
                        id: "Zone Encoder".to_owned(),
                        completion: SequenceCompletion::Frames {
                            completed: (progress.frame as u64).min(total_frames as u64),
                            total: total_frames as u64,
                        },
                    }))?;
                }
            }
            Ok(())
        });

        let (frames_tx, frames_rx) = crossbeam_channel::bounded::<Cursor<Vec<u8>>>(8);
        // A filtergraph applies to the whole pass, so every scene must agree
        // on it — `differing_field` already rejected the pass otherwise.
        let (encoder_rx, mut filter_stage) = match encoder.ffmpeg_filter() {
            Some(graph) => {
                let mut stage = FfmpegFilter::spawn(
                    frames_rx,
                    8,
                    graph,
                    Arc::new(Mutex::new(None)),
                    &cancelled,
                )?;
                (stage.take_receiver()?, Some(stage))
            },
            None => (frames_rx, None),
        };
        let encode_thread = thread::spawn({
            let encoder = encoder.clone();
            let whole_output = whole_output.clone();
            let cancelled = cancelled.clone();
            move || {
                let encoded = encoder.encode_with_stream(
                    encoder_rx,
                    &whole_output,
                    encode_progress_tx,
                    &clip_info,
                    &cancelled,
                );
                let filtered = filter_stage.as_mut().map_or(Ok(()), |stage| stage.check());
                (encoded, filtered)
            }
        });

        // Feeding by hand (rather than `y4m_frames`) keeps cancellation
        // responsive: the flag is checked between frames, a full channel
        // blocks only while the encoder lags, and a dead encoder shows up as a
        // failed send.
        let mut feed_failure = cancelled.send(&frames_tx, header).err();
        'feed: for task in &tasks {
            if feed_failure.is_some() {
                break;
            }
            for &index in &task.frame_indices {
                if cancelled.is_forced() {
                    break 'feed;
                }
                match reader.y4m_frame(index) {
                    Ok(frame) => {
                        if cancelled.send(&frames_tx, frame).is_err() {
                            break 'feed;
                        }
                    },
                    Err(error) => {
                        feed_failure = Some(error);
                        break 'feed;
                    },
                }
            }
        }
        drop(frames_tx); // EOF for the encoder
        drop(reader);

        let (encoded, filtered) = encode_thread.join().expect("zone encoder thread panicked");
        // The relay ends once the encode thread drops its progress sender.
        relay.join().expect("zone progress relay should join")?;

        if cancelled.is_forced() {
            discard();
            return crate::core::encoder::cancellation_result(
                encoded.map(|_| vec![None; total_tasks]),
                &cancelled,
            );
        }
        if let Err(err) = filtered {
            discard();
            bail!("FFmpeg filter failed while zone encoding: {err}");
        }
        if let Some(error) = feed_failure {
            discard();
            bail!(error);
        }
        let result = match encoded.or_else(anyhow::Error::downcast::<EncoderResult>) {
            Ok(result) => result,
            Err(error) => {
                discard();
                bail!(error);
            },
        };
        if !result.status.success() {
            let error = ParallelEncoderError::EncoderFailed {
                scene: first.original_index,
                result,
            };
            discard();
            bail!(error);
        }
        let ended = SystemTime::now();

        let split = match encoder.container(&cancelled)? {
            Some(Container::Ivf) => split_ivf(
                &whole_output,
                tasks.make_contiguous(),
                extension,
                &cancelled,
                validate_frame_count,
            ),
            Some(Container::AnnexB) => {
                let codec = match first.encoder.base() {
                    EncoderBase::X264 => Codec::H264,
                    _ => Codec::H265,
                };
                split_annexb(
                    &whole_output,
                    codec,
                    tasks.make_contiguous(),
                    extension,
                    &cancelled,
                    validate_frame_count,
                )
            },
            None => {
                discard();
                bail!("the encoder writes a container that cannot be split per scene");
            },
        };
        let split = match split {
            Ok(split) => split,
            Err(error) => {
                discard();
                return Err(error);
            },
        };
        let results = tasks
            .iter()
            .zip(split)
            .map(|(task, (bytes, frames))| {
                let bitrate = super::parallel_encoder::checked_bitrate(bytes, frames, framerate)
                    .with_context(|| format!("Scene {} split zone bitrate", task.original_index))?;
                Ok(Some(ParallelEncoderResult {
                    scene: task.original_index,
                    started,
                    ended,
                    bytes,
                    bitrate,
                    result: result.clone(),
                }))
            })
            .collect::<Result<Vec<_>>>();
        discard();
        let results = results?;

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: "Zone Encoder".to_owned(),
        }))?;
        partial.complete()?;
        Ok(results)
    }
}

/// The stream format of a zone-encoded pass, by output extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    H264,
    H265,
}

/// Byte offset of the IVF header's frame count field.
const IVF_NFRAME_OFFSET: usize = 24;

fn split_ivf(
    whole_output: &Path,
    tasks: &[Task],
    extension: &str,
    cancellation: &Cancellation,
    validate_frame_count: bool,
) -> Result<Vec<(u64, usize)>> {
    let mut reader = BufReader::new(
        fs::File::open(whole_output)
            .with_context(|| format!("failed to open {}", whole_output.display()))?,
    );
    let mut header = [0u8; 32];
    reader
        .read_exact(&mut header)
        .context("zone output is truncated before its IVF header")?;
    if &header[..4] != b"DKIF" {
        bail!(
            "zone output is not IVF (starts with {:02x?})",
            &header[..header.len().min(8)]
        );
    }
    if header[4..8] != [0, 0, 32, 0] {
        bail!("zone output requires a standard version 0, 32-byte IVF header");
    }
    if u32::from_le_bytes(header[16..20].try_into().expect("4 bytes")) == 0
        || u32::from_le_bytes(header[20..24].try_into().expect("4 bytes")) == 0
    {
        bail!("zone output has an invalid zero IVF rate/scale");
    }

    let mut parts: Vec<(PathBuf, u64, usize)> = Vec::with_capacity(tasks.len());
    let mut partials = Vec::new();
    let mut outputs = Vec::new();
    let mut previous_timestamp = None;
    for task in tasks {
        cancellation.check_forced()?;
        let expected = task.frame_indices.len();
        // Each part declares only its own frames: the whole-pass count would be
        // summed as duration when the scenes are concatenated back together.
        let count = u32::try_from(expected).with_context(|| {
            format!("scene {} IVF frame count exceeds u32", task.original_index)
        })?;
        header[IVF_NFRAME_OFFSET..IVF_NFRAME_OFFSET + 4].copy_from_slice(&count.to_le_bytes());
        let temp = task.output.with_extension(format!("temp.{extension}"));
        outputs.push(super::parallel_encoder::TempOutput::new(temp.clone()));
        partials.push(crate::core::process::InterruptedOutput::new(
            &temp,
            cancellation,
        ));
        let mut output = BufWriter::new(
            fs::File::create(&temp)
                .with_context(|| format!("failed to create {}", temp.display()))?,
        );
        output.write_all(&header)?;
        let mut bytes = 32u64;
        let mut frames = 0usize;
        let mut origin = None;
        while frames < expected {
            cancellation.check_forced()?;
            let mut frame_header = [0u8; 12];
            reader.read_exact(&mut frame_header).with_context(|| {
                format!(
                    "zone output is truncated before frame {frames} of scene {}",
                    task.original_index
                )
            })?;
            let size = u32::from_le_bytes(frame_header[..4].try_into().expect("4 bytes"));
            let timestamp = u64::from_le_bytes(frame_header[4..12].try_into().expect("8 bytes"));
            if previous_timestamp.is_some_and(|previous| timestamp <= previous) {
                bail!(
                    "zone IVF timestamp is not increasing at frame {frames} of scene {}",
                    task.original_index
                );
            }
            previous_timestamp = Some(timestamp);
            let local =
                timestamp.checked_sub(*origin.get_or_insert(timestamp)).with_context(|| {
                    format!(
                        "zone IVF timestamp underflow in scene {}",
                        task.original_index
                    )
                })?;
            frame_header[4..12].copy_from_slice(&local.to_le_bytes());
            output.write_all(&frame_header)?;
            let copied = std::io::copy(&mut reader.by_ref().take(size.into()), &mut output)?;
            if copied != u64::from(size) {
                bail!(
                    "zone output is truncated in frame {frames} of scene {}",
                    task.original_index
                );
            }
            bytes =
                bytes.checked_add(12 + u64::from(size)).context("zone IVF part size overflow")?;
            frames += 1;
        }
        output.flush()?;
        parts.push((temp, bytes, frames));
    }
    let mut trailing = [0u8; 1];
    if reader.read(&mut trailing)? != 0 {
        bail!("zone output has more frames than the scenes expect");
    }
    validate_parts(&parts, tasks, validate_frame_count, cancellation)?;
    cancellation.publish(|| {
        for ((part, _, _), task) in parts.iter().zip(tasks) {
            fs::rename(part, &task.output)
                .with_context(|| format!("failed to save {}", task.output.display()))?;
        }
        for partial in &mut partials {
            partial.disarm();
        }
        Ok(())
    })?;
    drop(partials);
    drop(outputs);
    Ok(parts.into_iter().map(|(_, bytes, frames)| (bytes, frames)).collect())
}

fn split_annexb(
    whole_output: &Path,
    codec: Codec,
    tasks: &[Task],
    extension: &str,
    cancellation: &Cancellation,
    validate_frame_count: bool,
) -> Result<Vec<(u64, usize)>> {
    let file = fs::File::open(whole_output)
        .with_context(|| format!("failed to open {}", whole_output.display()))?;
    let mut nals = NalReader::new(BufReader::new(file));

    let mut prefix: Vec<u8> = Vec::new();
    let mut prefix_complete = false;
    let mut parts: Vec<(PathBuf, u64, usize)> = Vec::with_capacity(tasks.len());
    let mut temp = tasks[0].output.with_extension(format!("temp.{extension}"));
    let mut outputs = vec![super::parallel_encoder::TempOutput::new(temp.clone())];
    let mut partials = vec![crate::core::process::InterruptedOutput::new(&temp, cancellation)];
    let mut output = new_part(&temp)?;
    let mut bytes = 0u64;
    let mut frames = 0usize;
    let mut part_index = 0usize;
    let mut picture_end = 0;

    while let Some(segment) = nals.next_segment()? {
        cancellation.check_forced()?;
        let frame_start = is_frame_start(codec, &segment);
        if frame_start && frames == tasks[part_index].frame_indices.len() {
            output.flush()?;
            part_index += 1;
            if part_index >= tasks.len() {
                bail!("zone output has more frames than the scenes expect");
            }
            let next_temp = tasks[part_index].output.with_extension(format!("temp.{extension}"));
            outputs.push(super::parallel_encoder::TempOutput::new(next_temp.clone()));
            partials.push(crate::core::process::InterruptedOutput::new(
                &next_temp,
                cancellation,
            ));
            let mut next = new_part(&next_temp)?;
            next.write_all(&prefix)?;
            // Parameter sets/AUD/prefix SEI before the next picture belong to
            // that part. Leaving them on the previous part produces a dangling
            // access unit that decoder-backed validation correctly rejects.
            // Copy the tail from disk instead of buffering arbitrary NAL metadata.
            let mut tail = fs::File::open(&temp)?;
            tail.seek(SeekFrom::Start(picture_end))?;
            std::io::copy(&mut tail.take(bytes - picture_end), &mut next)?;
            output.get_ref().set_len(picture_end)?;
            drop(output);
            parts.push((temp, picture_end, frames));
            temp = next_temp;
            output = next;
            bytes = prefix.len() as u64 + bytes - picture_end;
            frames = 0;
        }
        if !prefix_complete {
            if frame_start {
                prefix_complete = true;
            } else {
                prefix.extend_from_slice(&segment);
            }
        }
        output.write_all(&segment)?;
        bytes += segment.len() as u64;
        let leading_header = segment.get(3).is_some_and(|header| match codec {
            Codec::H264 => matches!(header & 0x1F, 6..=9),
            Codec::H265 => matches!((header >> 1) & 0x3F, 32..=35 | 39),
        });
        if !leading_header {
            picture_end = bytes;
        }
        if frame_start {
            frames += 1;
        }
    }
    if !prefix_complete {
        bail!("zone output contains no frames");
    }
    output.flush()?;
    drop(output); // closes the last part before it is renamed
    parts.push((temp, bytes, frames));

    if parts.len() != tasks.len() {
        bail!("zone output has fewer frames than the scenes expect");
    }
    for ((_, _, frames), task) in parts.iter().zip(tasks) {
        if *frames != task.frame_indices.len() {
            bail!(
                "zone output has {frames} frames for scene {} which expects {}",
                task.original_index,
                task.frame_indices.len()
            );
        }
    }
    validate_parts(&parts, tasks, validate_frame_count, cancellation)?;
    cancellation.publish(|| {
        for (part, task) in parts.iter().zip(tasks) {
            fs::rename(&part.0, &task.output)
                .with_context(|| format!("failed to save {}", task.output.display()))?;
        }
        for partial in &mut partials {
            partial.disarm();
        }
        Ok(())
    })?;
    drop(partials);
    drop(outputs);
    Ok(parts.into_iter().map(|(_, bytes, frames)| (bytes, frames)).collect())
}

fn validate_parts(
    parts: &[(PathBuf, u64, usize)],
    tasks: &[Task],
    count: bool,
    token: &Cancellation,
) -> Result<()> {
    for ((path, _, _), task) in parts.iter().zip(tasks) {
        super::parallel_encoder::artifact::protect_existing(&task.output)?;
        super::parallel_encoder::artifact::validate(path, task.frame_indices.len(), count, token)
            .with_context(|| format!("Scene {} zone output", task.original_index))?;
    }
    Ok(())
}

#[inline]
fn new_part(temp: &Path) -> Result<BufWriter<fs::File>> {
    Ok(BufWriter::new(fs::File::create(temp).with_context(
        || format!("failed to create {}", temp.display()),
    )?))
}

/// Whether an Annex-B segment opens a new coded picture: a VCL NAL whose
/// first syntax element says it starts a picture (`first_mb_in_slice` /
/// `first_slice_segment_in_pic_flag` set). Non-VCL segments (SPS, PPS, SEI,
/// AUD) and slice continuations do not count, so each picture is counted once
/// regardless of how many NAL units it uses.
fn is_frame_start(codec: Codec, segment: &[u8]) -> bool {
    if segment.len() < 5 || !segment.starts_with(&[0, 0, 1]) {
        return false;
    }
    match codec {
        Codec::H264 => {
            // nal_unit_type 1 (non-IDR slice) or 5 (IDR); then the MSB of the
            // first RBSP byte is first_mb_in_slice == 0 (ue(v) 0 starts with
            // 1).
            matches!(segment[3] & 0x1F, 1 | 5) && (segment[4] & 0x80) != 0
        },
        Codec::H265 => {
            if segment.len() < 6 {
                return false;
            }
            // nal_unit_type 0..=21 are VCL; the MSB of the byte after the
            // two-byte header is first_slice_segment_in_pic_flag.
            (segment[3] >> 1) & 0x3F <= 21 && (segment[5] & 0x80) != 0
        },
    }
}

/// Splits an Annex-B byte stream into segments: everything before each
/// successive `00 00 01` start code, then the start code with the bytes that
/// follow it. Concatenating the segments reproduces the stream exactly, so
/// splitting here cannot corrupt the bitstream. One segment (at most one NAL
/// unit) is buffered at a time.
struct NalReader<R: BufRead> {
    inner: R,
    pending: Vec<u8>,
    /// Index in `pending` of the start code opening the segment being
    /// collected, once one has been found.
    current: Option<usize>,
    /// Where to resume the start-code scan while `current` is `None`.
    searched: usize,
    eof: bool,
}

impl<R: BufRead> NalReader<R> {
    #[inline]
    fn new(inner: R) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            current: None,
            searched: 0,
            eof: false,
        }
    }

    fn next_segment(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if self.current.is_none() {
                if let Some(start) = find_start_code(&self.pending, self.searched) {
                    self.current = Some(start);
                } else {
                    // Every complete start code from `searched` onward was
                    // checked; once more data arrives one can only begin in
                    // the last two bytes of what has been read so far.
                    self.searched = self.pending.len().saturating_sub(2);
                }
            }
            if let Some(start) = self.current
                && let Some(end) = find_start_code(&self.pending, start + 3)
            {
                let segment = self.pending.drain(..end).collect();
                self.current = None;
                self.searched = 0;
                return Ok(Some(segment));
            }
            if self.eof {
                if let Some(start) = self.current.take() {
                    let segment = self.pending.drain(start..).collect();
                    self.searched = 0;
                    return Ok(Some(segment));
                }
                if !self.pending.is_empty() {
                    return Ok(Some(std::mem::take(&mut self.pending)));
                }
                return Ok(None);
            }
            let buffer = self.inner.fill_buf()?;
            if buffer.is_empty() {
                self.eof = true;
            } else {
                self.pending.extend_from_slice(buffer);
                let read = buffer.len();
                self.inner.consume(read);
            }
        }
    }
}

/// First index at or after `from` where `00 00 01` occurs.
#[inline]
fn find_start_code(bytes: &[u8], from: usize) -> Option<usize> {
    bytes
        .windows(3)
        .skip(from)
        .position(|window| window == [0, 0, 1])
        .map(|offset| offset + from)
}

#[cfg(test)]
mod tests;
