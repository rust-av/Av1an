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
    io::{BufRead, BufReader, BufWriter, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        self,
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use thiserror::Error;
use tracing::debug;

use crate::{
    core::{
        encoder::{Container, EncodeProgress, EncoderCapability},
        input::Input,
        sequence::{
            SequenceCompletion,
            SequenceStatus,
            Status,
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
    pub start:     usize,
    /// Last stream frame of the zone (inclusive).
    pub end:       usize,
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
    zones:          Vec<Zone>,
    /// `--zones` value (x264) or zonefile body (x265).
    zone_option:    String,
    /// qpfile body forcing an IDR at every zone start past the first; empty
    /// when none are needed.
    qpfile_content: String,
    /// SVT-AV1 config file body (`Name : value` lines), which carries its
    /// zones and keyframes instead of the command line.
    config_content: String,
}

impl ZonePlan {
    /// Builds the zoning plan for a pass, or explains why the pass must fall
    /// back to parallel encoding.
    ///
    /// Checks are ordered cheap-first; the capability probe (which spawns the
    /// encoder binary) runs only when everything else already holds.
    #[inline]
    pub fn try_build(tasks: &[Task]) -> Result<ZonePlan, ZoneIneligible> {
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

        if !first.encoder.supports_capability(EncoderCapability::Zoning) {
            return Err(ZoneIneligible::NoZoneSupport);
        }
        // SVT-AV1 is probed with `--webm 0` (see [`apply_zone_options`]), so a
        // build that writes Matroska still reports the IVF it will produce.
        // A build whose output cannot be split per scene is ineligible.
        if first.encoder.container().is_none() {
            return Err(ZoneIneligible::UnsplittableContainer);
        }

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
    /// Encodes every task in one zoned process and splits the output back
    /// into the tasks' files, mirroring
    /// [`ParallelEncoder::encode_tasks`](super::parallel_encoder::ParallelEncoder::encode_tasks).
    ///
    /// The owned arguments come from that mirrored signature: the pass hands
    /// over its tasks, plan and channels rather than borrowing them.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::needless_pass_by_value)]
    #[inline]
    pub fn encode_tasks(
        input: &mut Input,
        mut tasks: VecDeque<Task>,
        plan: ZonePlan,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        let total_tasks = tasks.len();
        let total_frames: usize = tasks.iter().map(|task| task.frame_indices.len()).sum();
        debug!("Zone encoding {total_tasks} scenes ({total_frames} frames) in one process");

        let first = tasks.front().expect("zone plan was built from tasks");
        let directory = first.output.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mut encoder = first.encoder.clone();
        let zone_files = apply_zone_options(&mut encoder, &plan, &directory)?;
        let extension = encoder.output_extension();
        let whole_output = directory.join(format!("zone-pass.temp.{extension}"));
        let discard = {
            let whole_output = whole_output.clone();
            move || {
                let _ = fs::remove_file(&whole_output);
                for path in &zone_files {
                    let _ = fs::remove_file(path);
                }
            }
        };

        let started = SystemTime::now();
        let header = Cursor::new(input.y4m_header(Some(total_frames))?.into_bytes());
        let clip_info = input.clip_info()?;

        // Encoder progress reports whole-stream frame counts; the receiver
        // side of the pass re-labels them "Encode".
        let (encode_progress_tx, encode_progress_rx) = mpsc::channel::<EncodeProgress>();
        let relay_tx = progress_tx.clone();
        let relay = thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                if progress.pass.0 == progress.pass.1 && progress.frame > 0 {
                    relay_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         "Zone Encoder".to_owned(),
                        completion: SequenceCompletion::Frames {
                            completed: (progress.frame as u64).min(total_frames as u64),
                            total:     total_frames as u64,
                        },
                    }))?;
                }
            }
            Ok(())
        });

        let (frames_tx, frames_rx) = crossbeam_channel::bounded::<Cursor<Vec<u8>>>(8);
        // A filtergraph applies to the whole pass, so every scene must agree
        // on it — `differing_field` already rejected the pass otherwise.
        let (encoder_rx, filter_stage) = match encoder.ffmpeg_filter() {
            Some(graph) => {
                let stage = FfmpegFilter::spawn(frames_rx, 8, graph, Arc::new(Mutex::new(None)))?;
                (stage.receiver(), Some(stage))
            },
            None => (frames_rx, None),
        };
        let encode_thread = thread::spawn({
            let encoder = encoder.clone();
            let whole_output = whole_output.clone();
            move || {
                encoder.encode_with_stream(
                    encoder_rx,
                    &whole_output,
                    encode_progress_tx,
                    &clip_info,
                )
            }
        });

        // Feeding by hand (rather than `y4m_frames`) keeps cancellation
        // responsive: the flag is checked between frames, a full channel
        // blocks only while the encoder lags, and a dead encoder shows up as a
        // failed send.
        frames_tx.send(header)?;
        let mut feed_failure = None;
        'feed: for task in &tasks {
            for &index in &task.frame_indices {
                if cancelled.load(Ordering::Relaxed) {
                    break 'feed;
                }
                match input.y4m_frame(index) {
                    Ok(frame) => {
                        if frames_tx.send(frame).is_err() {
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

        let encoded = encode_thread.join().expect("zone encoder thread panicked");
        // The relay ends once the encode thread drops its progress sender.
        relay.join().expect("zone progress relay should join")?;

        if cancelled.load(Ordering::Relaxed) {
            discard();
            return Ok(vec![None; total_tasks]);
        }
        if let Some(stage) = &filter_stage
            && let Err(err) = stage.check()
        {
            discard();
            bail!("FFmpeg filter failed while zone encoding: {err}");
        }
        if let Some(error) = feed_failure {
            discard();
            bail!(error);
        }
        let result = match encoded {
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

        let split = match encoder.container() {
            Some(Container::Ivf) => split_ivf(&whole_output, tasks.make_contiguous(), extension),
            Some(Container::AnnexB) => {
                let codec = match first.encoder.base() {
                    EncoderBase::X264 => Codec::H264,
                    _ => Codec::H265,
                };
                split_annexb(&whole_output, codec, tasks.make_contiguous(), extension)
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
        let framerate = {
            let clip_info = input.clip_info()?;
            *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64
        };
        let results = tasks
            .iter()
            .zip(split)
            .map(|(task, (bytes, frames))| {
                let seconds = frames as f64 / framerate;
                Some(ParallelEncoderResult {
                    scene: task.original_index,
                    started,
                    ended,
                    bytes,
                    bitrate: (bytes * 8) as f64 / seconds,
                    result: result.clone(),
                })
            })
            .collect();
        discard();

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: "Zone Encoder".to_owned(),
        }))?;
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
const IVF_NFRAME_OFFSET: usize = 22;

/// Splits a whole-pass IVF file into one file per task, each with the shared
/// 32-byte file header. Returns `(bytes, frames)` per task.
fn split_ivf(whole_output: &Path, tasks: &[Task], extension: &str) -> Result<Vec<(u64, usize)>> {
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

    let mut parts: Vec<(PathBuf, u64, usize)> = Vec::with_capacity(tasks.len());
    for task in tasks {
        let expected = task.frame_indices.len();
        // Each part declares only its own frames: the whole-pass count would be
        // summed as duration when the scenes are concatenated back together.
        header[IVF_NFRAME_OFFSET..IVF_NFRAME_OFFSET + 4]
            .copy_from_slice(&(expected as u32).to_le_bytes());
        let temp = task.output.with_extension(format!("temp.{extension}"));
        let mut output = BufWriter::new(
            fs::File::create(&temp)
                .with_context(|| format!("failed to create {}", temp.display()))?,
        );
        output.write_all(&header)?;
        let mut bytes = 32u64;
        let mut frames = 0usize;
        while frames < expected {
            let mut frame_header = [0u8; 12];
            reader.read_exact(&mut frame_header).with_context(|| {
                format!(
                    "zone output is truncated before frame {frames} of scene {}",
                    task.original_index
                )
            })?;
            let size = u32::from_le_bytes(frame_header[..4].try_into().expect("4 bytes"));
            output.write_all(&frame_header)?;
            let copied = std::io::copy(&mut reader.by_ref().take(size.into()), &mut output)?;
            if copied != u64::from(size) {
                bail!(
                    "zone output is truncated in frame {frames} of scene {}",
                    task.original_index
                );
            }
            bytes += 12 + u64::from(size);
            frames += 1;
        }
        output.flush()?;
        parts.push((temp, bytes, frames));
    }
    let mut trailing = [0u8; 1];
    if reader.read(&mut trailing)? != 0 {
        bail!("zone output has more frames than the scenes expect");
    }
    for ((part, _, _), task) in parts.iter().zip(tasks) {
        fs::rename(part, &task.output)
            .with_context(|| format!("failed to save {}", task.output.display()))?;
    }
    Ok(parts.into_iter().map(|(_, bytes, frames)| (bytes, frames)).collect())
}

/// Splits a whole-pass Annex-B stream into one file per task. Each file gets
/// a copy of the headers (SPS/PPS/SEI) that precede the first frame, so every
/// part decodes standalone like a per-scene encode would.
fn split_annexb(
    whole_output: &Path,
    codec: Codec,
    tasks: &[Task],
    extension: &str,
) -> Result<Vec<(u64, usize)>> {
    let file = fs::File::open(whole_output)
        .with_context(|| format!("failed to open {}", whole_output.display()))?;
    let mut nals = NalReader::new(BufReader::new(file));

    let mut prefix: Vec<u8> = Vec::new();
    let mut prefix_complete = false;
    let mut parts: Vec<(PathBuf, u64, usize)> = Vec::with_capacity(tasks.len());
    let mut temp = tasks[0].output.with_extension(format!("temp.{extension}"));
    let mut output = new_part(&temp)?;
    let mut bytes = 0u64;
    let mut frames = 0usize;
    let mut part_index = 0usize;

    while let Some(segment) = nals.next_segment()? {
        let frame_start = is_frame_start(codec, &segment);
        if frame_start && frames == tasks[part_index].frame_indices.len() {
            output.flush()?;
            parts.push((temp, bytes, frames));
            part_index += 1;
            if part_index >= tasks.len() {
                bail!("zone output has more frames than the scenes expect");
            }
            temp = tasks[part_index].output.with_extension(format!("temp.{extension}"));
            output = new_part(&temp)?;
            output.write_all(&prefix)?;
            bytes = prefix.len() as u64;
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
    for (part, task) in parts.iter().zip(tasks) {
        fs::rename(&part.0, &task.output)
            .with_context(|| format!("failed to save {}", task.output.display()))?;
    }
    Ok(parts.into_iter().map(|(_, bytes, frames)| (bytes, frames)).collect())
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
            // first RBSP byte is first_mb_in_slice == 0 (ue(v) 0 starts with 1).
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
    inner:    R,
    pending:  Vec<u8>,
    /// Index in `pending` of the start code opening the segment being
    /// collected, once one has been found.
    current:  Option<usize>,
    /// Where to resume the start-code scan while `current` is `None`.
    searched: usize,
    eof:      bool,
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
mod tests {
    use super::*;
    use crate::models::encoder::photon_noise::PhotonNoise;

    fn temp_directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("av1an-zone-encoder-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("temp directory created");
        directory
    }

    fn x264(quantizer: f64) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_quantizer(quantizer);
        encoder
    }

    fn x265_encoder(quantizer: f64) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        encoder.set_quantizer(quantizer);
        encoder
    }

    fn svt_av1(quantizer: f64) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder.set_quantizer(quantizer);
        encoder
    }

    /// Sets the photon noise the noise scaler would have written for a scene.
    fn set_photon_noise_iso(encoder: &mut Encoder, iso: u32) {
        let Encoder::SVTAV1 {
            photon_noise, ..
        } = encoder
        else {
            panic!("only SVT-AV1 carries photon noise");
        };
        *photon_noise = Some(PhotonNoise {
            iso,
            chroma_iso: Some(iso / 3),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        });
    }

    fn task(original_index: usize, frames: usize, encoder: Encoder) -> Task {
        Task {
            original_index,
            index: original_index,
            frame_indices: (0..frames).collect(),
            sub_scenes: None,
            encoder,
            output: PathBuf::from(format!("scene{original_index}.264")),
        }
    }

    #[test]
    fn zones_cover_every_frame_of_every_scene() {
        let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, x264(30.0)), task(2, 7, x264(40.5))];
        let zones = ZonePlan::from_tasks(&tasks).expect("zones built");
        assert_eq!(zones, vec![
            Zone {
                start:     0,
                end:       4,
                quantizer: 20.0,
            },
            Zone {
                start:     5,
                end:       7,
                quantizer: 30.0,
            },
            Zone {
                start:     8,
                end:       14,
                quantizer: 40.5,
            },
        ]);
    }

    #[test]
    fn a_scene_without_a_quantizer_is_ineligible() {
        let mut scene = x264(30.0);
        scene.parameters_mut().remove("crf");
        let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, scene)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::NoQuantizer {
                scene: 1
            })
        );
    }

    #[test]
    fn encoders_differing_beyond_the_quantizer_are_ineligible() {
        let mut scene = x264(30.0);
        scene.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "fast"),
        );
        let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, scene)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::DifferingEncoder {
                scene: 1,
                field: DifferingField::Options,
            })
        );
    }

    #[test]
    fn a_different_encoder_is_ineligible() {
        let mut other = Encoder::default_from_base(&EncoderBase::X265, false);
        other.set_quantizer(30.0);
        let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, other)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::DifferingEncoder {
                scene: 1,
                field: DifferingField::Base,
            })
        );
    }

    #[test]
    fn a_shared_filter_keeps_the_pass_zone_eligible() {
        let mut first = x264(20.0);
        first.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        let mut second = x264(30.0);
        second.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        let tasks = vec![task(0, 5, first), task(1, 3, second)];
        assert!(
            ZonePlan::try_build(&tasks).is_ok(),
            "scenes sharing one filtergraph still zone encode"
        );
    }

    #[test]
    fn differing_filters_are_ineligible() {
        let mut first = x264(20.0);
        first.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        let mut second = x264(30.0);
        second.set_ffmpeg_filter(Some("hflip".to_owned()));
        let tasks = vec![task(0, 5, first), task(1, 3, second)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::DifferingEncoder {
                scene: 1,
                field: DifferingField::FfmpegFilter,
            })
        );
    }

    #[test]
    fn a_filter_on_only_one_scene_is_ineligible() {
        let mut filtered = x264(30.0);
        filtered.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, filtered)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::DifferingEncoder {
                scene: 1,
                field: DifferingField::FfmpegFilter,
            })
        );
    }

    /// The zone encoder compares photon noise, so a scene the noise scaler
    /// gave a stronger iso must be reported as a photon-noise difference, and
    /// the fallback log must name the field.
    #[test]
    fn scenes_differing_only_in_photon_noise_name_the_field() {
        let mut first = svt_av1(20.0);
        set_photon_noise_iso(&mut first, 600);
        let mut second = svt_av1(30.0);
        set_photon_noise_iso(&mut second, 12000);
        let tasks = vec![task(0, 5, first), task(1, 3, second)];
        let reason = ZonePlan::try_build(&tasks).expect_err("scenes differ");
        assert_eq!(reason, ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::PhotonNoise,
        });
        assert_eq!(
            reason.to_string(),
            "scene 1 differs from the first scene beyond the quantizer: photon_noise"
        );
    }

    /// Equality is structural: two photon noises that serialize identically
    /// are not a difference, so the quantizer remains the only thing scenes
    /// may vary in.
    #[test]
    fn identical_photon_noise_is_not_a_difference() {
        let mut first = svt_av1(20.0);
        set_photon_noise_iso(&mut first, 6177);
        let mut second = svt_av1(30.0);
        set_photon_noise_iso(&mut second, 6177);
        assert_eq!(
            differing_field(&first, &second),
            None,
            "identical photon noise leaves the quantizer as the only difference"
        );
    }

    #[test]
    fn the_ffmpeg_encoder_never_carries_a_filter() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
        encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        assert_eq!(
            encoder.ffmpeg_filter(),
            None,
            "the FFmpeg encoder filters through its options instead"
        );
    }

    #[test]
    fn multi_pass_encodes_are_ineligible() {
        let mut encoder = x264(20.0);
        *encoder.passes_mut().expect("x264 has passes") = EncoderPasses::All(2);
        let tasks = vec![task(0, 5, encoder)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::NotSinglePass {
                scene: 0
            })
        );
    }

    #[test]
    fn user_configured_zone_options_are_ineligible() {
        let mut encoder = x264(20.0);
        encoder.parameters_mut().insert(
            "qpfile".to_owned(),
            CLIParameter::new_string("--", " ", "mine.txt"),
        );
        let tasks = vec![task(0, 5, encoder)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::ReservedOption {
                scene: 0
            })
        );
    }

    #[test]
    fn unsupported_encoders_are_ineligible() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        encoder.set_quantizer(30.0);
        let tasks = vec![task(0, 5, encoder)];
        assert_eq!(
            ZonePlan::try_build(&tasks),
            Err(ZoneIneligible::UnsupportedEncoder(EncoderBase::AOM))
        );
    }

    #[test]
    fn empty_passes_are_ineligible() {
        assert_eq!(ZonePlan::try_build(&[]), Err(ZoneIneligible::Empty));
    }

    #[test]
    fn x264_options_carry_zones_and_keyframes() {
        let zones = ZonePlan::from_tasks(&[
            task(0, 5, x264(20.0)),
            task(1, 3, x264(30.0)),
            task(2, 7, x264(40.5)),
        ])
        .expect("zones built");
        let (zone_option, qpfile_content, config_content) =
            format_zone_options(EncoderBase::X264, &zones);
        assert_eq!(zone_option, "0,4,crf=20/5,7,crf=30/8,14,crf=40.5");
        assert_eq!(qpfile_content, "5 I\n8 I\n");
        assert_eq!(config_content, "");
    }

    #[test]
    fn x265_options_write_one_zonefile_line_per_scene() {
        let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
            .expect("zones built");
        let (zone_option, qpfile_content, config_content) =
            format_zone_options(EncoderBase::X265, &zones);
        assert_eq!(zone_option, "0 --crf 20\n5 --crf 30\n");
        assert_eq!(qpfile_content, "5 I\n");
        assert_eq!(config_content, "");
    }

    /// SVT-AV1 carries its zones and keyframes in a config file, keyed on the
    /// option's descriptive name.
    #[test]
    fn svt_options_write_a_config_file_keyed_by_option_name() {
        let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
            .expect("zones built");
        let (zone_option, qpfile_content, config_content) =
            format_zone_options(EncoderBase::SVTAV1, &zones);
        assert_eq!(zone_option, "");
        assert_eq!(qpfile_content, "");
        assert_eq!(
            config_content,
            "Zones : 0,4,20;5,7,30\nForceKeyFrames : 5f\n"
        );
    }

    #[test]
    fn a_single_scene_needs_no_keyframe_options() {
        let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0))]).expect("zones built");
        let (zone_option, qpfile_content, config_content) =
            format_zone_options(EncoderBase::X264, &zones);
        assert_eq!(zone_option, "0,4,crf=20");
        assert_eq!(qpfile_content, "");
        assert_eq!(config_content, "");
        let (_, qpfile_content, config_content) = format_zone_options(EncoderBase::SVTAV1, &zones);
        assert_eq!(qpfile_content, "");
        // No zone starts past frame 0, so no keyframe list is needed.
        assert_eq!(config_content, "Zones : 0,4,20\n");
    }

    #[test]
    fn zone_options_are_written_next_to_the_scene_files() {
        let directory = temp_directory("options");
        let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
            .expect("zones built");
        let plan = ZonePlan {
            zone_option: "0,4,crf=20/5,7,crf=30".to_owned(),
            qpfile_content: "5 I\n".to_owned(),
            config_content: String::new(),
            zones,
        };
        let mut encoder = x264(0.0);
        let zone_files =
            apply_zone_options(&mut encoder, &plan, &directory).expect("options applied");
        let zones = encoder.parameters().get("zones").expect("zones option");
        let CLIParameter::String {
            value: zones_value,
            ..
        } = zones
        else {
            panic!("zones option should be a string");
        };
        assert_eq!(zones_value, "0,4,crf=20/5,7,crf=30");
        assert_eq!(zone_files.len(), 1);
        let qpfile = fs::read_to_string(&zone_files[0]).expect("qpfile written");
        assert_eq!(qpfile, "5 I\n");
        let qpfile_option = encoder.parameters().get("qpfile").expect("qpfile option");
        let CLIParameter::String {
            value: qpfile_value,
            ..
        } = qpfile_option
        else {
            panic!("qpfile option should be a string");
        };
        assert_eq!(qpfile_value, &zone_files[0].display().to_string());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn frame_starts_ignore_headers_and_slice_continuations() {
        // IDR with first_mb_in_slice == 0 (MSB of the first RBSP byte set).
        assert!(is_frame_start(Codec::H264, &[0, 0, 1, 0x65, 0x88]));
        // P slice starting a picture.
        assert!(is_frame_start(Codec::H264, &[0, 0, 1, 0x41, 0x80]));
        // P slice continuing a picture (first_mb_in_slice != 0).
        assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x41, 0x00]));
        // Access unit delimiter and SPS are not frames.
        assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x09, 0x10]));
        assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x67, 0x42]));
        // Trailing zeros alone are not a frame.
        assert!(!is_frame_start(Codec::H264, &[0, 0, 0, 0, 0]));

        // IDR_W_RADL (type 19) with first_slice_segment_in_pic_flag set.
        assert!(is_frame_start(Codec::H265, &[0, 0, 1, 0x26, 0x01, 0x80]));
        // TRAIL_R (type 1) starting a picture.
        assert!(is_frame_start(Codec::H265, &[0, 0, 1, 0x02, 0x01, 0x80]));
        // TRAIL_R continuing a picture.
        assert!(!is_frame_start(Codec::H265, &[0, 0, 1, 0x02, 0x01, 0x01]));
        // VPS (type 32) is not a frame.
        assert!(!is_frame_start(Codec::H265, &[0, 0, 1, 0x40, 0x01, 0x80]));
    }

    /// A real zone-encoded stream must split back into decodable parts.
    ///
    /// The synthetic stream cannot catch the mistakes that matter most: a
    /// boundary landing inside a slice instead of at a picture, or the
    /// per-file headers missing so a part no longer decodes. This runs the
    /// actual x264 binary when it is available.
    #[test]
    fn a_real_zone_stream_splits_into_decodable_parts() {
        let Ok(x264_binary) = which::which("x264") else {
            return; // no encoder available to test against
        };
        let directory = temp_directory("real");
        let source = directory.join("full.y4m");
        // 30 frames of a cheap gradient, 10 per scene.
        write_test_y4m(&source, 0, 30, 8);

        let scene_frames = [10usize, 10, 10];
        let crfs = [20u32, 40, 30];
        // Each scene encoded on its own, as Parallel Encoder does.
        for (index, crf) in crfs.iter().enumerate() {
            let scene = directory.join(format!("par{index}.y4m"));
            write_test_y4m(&scene, index * 10, 10, 8);
            let output = directory.join(format!("par{index}.264"));
            let status = std::process::Command::new(&x264_binary)
                .args([
                    "--demuxer",
                    "y4m",
                    "--keyint",
                    "infinite",
                    "--scenecut",
                    "0",
                    "--preset",
                    "ultrafast",
                    "--log-level",
                    "error",
                    "--crf",
                    &crf.to_string(),
                    "-o",
                    output.to_str().expect("output path"),
                    scene.to_str().expect("scene path"),
                ])
                .status()
                .expect("x264 runs");
            assert!(status.success(), "per-scene encode {index} succeeded");
        }

        // The whole pass encoded once, zoned, with IDRs at the scene starts.
        let qpfile = directory.join("qpf");
        fs::write(&qpfile, "10 I\n20 I\n").expect("qpfile written");
        let whole = directory.join("zone.264");
        let status = std::process::Command::new(&x264_binary)
            .args([
                "--demuxer",
                "y4m",
                "--keyint",
                "infinite",
                "--scenecut",
                "0",
                "--preset",
                "ultrafast",
                "--log-level",
                "error",
                "--crf",
                "25",
                "--zones",
                "0,9,crf=20/10,19,crf=40/20,29,crf=30",
                "--qpfile",
                qpfile.to_str().expect("qpfile path"),
                "-o",
                whole.to_str().expect("output path"),
                source.to_str().expect("source path"),
            ])
            .status()
            .expect("x264 runs");
        assert!(status.success(), "zoned encode succeeded");

        let tasks: Vec<Task> = scene_frames
            .iter()
            .enumerate()
            .map(|(index, frames)| Task {
                original_index: index,
                index,
                frame_indices: (0..*frames).collect(),
                sub_scenes: None,
                encoder: x264(crfs[index] as f64),
                output: directory.join(format!("part{index}.264")),
            })
            .collect();
        let split = split_annexb(&whole, Codec::H264, &tasks, "264").expect("split");
        assert_eq!(
            split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
            scene_frames.to_vec(),
            "each part holds its scene's frames"
        );

        // Every part decodes on its own, to the same picture as the independent
        // encode it stands in for.
        let ffmpeg = which::which("ffmpeg").ok();
        for index in 0..scene_frames.len() {
            let part = fs::read(&tasks[index].output).expect("part readable");
            assert!(
                part.starts_with(&[0x00, 0x00, 0x00, 0x01, 0x67]),
                "part {index} opens with the stream headers"
            );
            let Some(ffmpeg) = &ffmpeg else {
                continue;
            };
            let mut decoded = Vec::new();
            for name in ["part", "par"] {
                let file = directory.join(format!("{name}{index}.264"));
                let raw = directory.join(format!("{name}{index}.raw"));
                let output = std::process::Command::new(ffmpeg)
                    .args([
                        "-v",
                        "error",
                        "-i",
                        file.to_str().expect("file path"),
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "yuv420p",
                        "-y",
                        raw.to_str().expect("raw path"),
                    ])
                    .output()
                    .expect("ffmpeg runs");
                let pixels = fs::read(&raw).expect("raw frames written");
                let _ = fs::remove_file(&raw);
                let frames = pixels.len() / (WIDTH_CY * HEIGHT_CY * 3 / 2);
                assert!(
                    output.status.success() && frames == scene_frames[index],
                    "{name}{index}.264 decoded {frames} frames, expected {}",
                    scene_frames[index]
                );
                decoded.push(pixels);
            }
            // The first zone starts from the encoder's initial state, so it
            // matches the independent encode exactly. Later zones inherit
            // rate-control state across the boundary, so they differ
            // slightly -- the cost of encoding the pass in one process.
            let (worst_delta, differing) = pixel_difference(&decoded[0], &decoded[1]);
            if index == 0 {
                assert_eq!(
                    differing, 0,
                    "the first zone must be pixel-identical to its own encode"
                );
            } else {
                assert!(
                    worst_delta <= MAX_RATE_CONTROL_CARRY_DELTA,
                    "part {index} differs from the independent encode by {worst_delta}, beyond \
                     rate-control carryover"
                );
            }
        }
        let _ = fs::remove_dir_all(&directory);
    }

    /// The same guarantee for x265, which splits on
    /// `first_slice_segment_in_pic_flag` and reconfigures the quantizer from a
    /// zone file rather than an inline option.
    #[test]
    fn a_real_x265_zone_stream_splits_into_decodable_parts() {
        let Ok(x265) = which::which("x265") else {
            return; // no encoder available to test against
        };
        let Ok(ffmpeg) = which::which("ffmpeg") else {
            return; // decoding is how the parts are verified
        };
        let directory = temp_directory("x265");
        let source = directory.join("full.y4m");
        write_test_y4m(&source, 0, 30, 8);

        let scene_frames = [10usize, 10, 10];
        let crfs = [20u32, 40, 30];
        for (index, crf) in crfs.iter().enumerate() {
            let scene = directory.join(format!("par{index}.y4m"));
            write_test_y4m(&scene, index * 10, 10, 8);
            let output = directory.join(format!("par{index}.hevc"));
            let status = std::process::Command::new(&x265)
                .args([
                    "--y4m",
                    "--keyint",
                    "-1",
                    "--scenecut",
                    "0",
                    "--preset",
                    "ultrafast",
                    "--log-level",
                    "error",
                    "--crf",
                    &crf.to_string(),
                    "-o",
                    output.to_str().expect("output path"),
                    scene.to_str().expect("scene path"),
                ])
                .status()
                .expect("x265 runs");
            assert!(status.success(), "per-scene encode {index} succeeded");
        }

        let qpfile = directory.join("qpf");
        fs::write(&qpfile, "10 I\n20 I\n").expect("qpfile written");
        let zonefile = directory.join("zonefile");
        // A zone line for every scene, including the first: without one, x265 leaves
        // the first zone on the global CRF rather than the scene's.
        let mut zonefile_content = String::new();
        for (index, crf) in crfs.iter().enumerate() {
            let _ = writeln!(zonefile_content, "{} --crf {}", index * 10, crf);
        }
        fs::write(&zonefile, zonefile_content).expect("zonefile written");
        let whole = directory.join("zone.hevc");
        let status = std::process::Command::new(&x265)
            .args([
                "--y4m",
                "--keyint",
                "-1",
                "--scenecut",
                "0",
                "--preset",
                "ultrafast",
                "--log-level",
                "error",
                "--crf",
                "25",
                "--zonefile",
                zonefile.to_str().expect("zonefile path"),
                "--qpfile",
                qpfile.to_str().expect("qpfile path"),
                "-o",
                whole.to_str().expect("output path"),
                source.to_str().expect("source path"),
            ])
            .status()
            .expect("x265 runs");
        assert!(status.success(), "zoned encode succeeded");

        let tasks: Vec<Task> = scene_frames
            .iter()
            .enumerate()
            .map(|(index, frames)| Task {
                original_index: index,
                index,
                frame_indices: (0..*frames).collect(),
                sub_scenes: None,
                encoder: x265_encoder(crfs[index] as f64),
                output: directory.join(format!("part{index}.hevc")),
            })
            .collect();
        let split = split_annexb(&whole, Codec::H265, &tasks, "hevc").expect("split");
        assert_eq!(
            split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
            scene_frames.to_vec(),
            "each part holds its scene's frames"
        );

        for (index, expected_frames) in scene_frames.iter().enumerate() {
            let mut decoded = Vec::new();
            for name in ["part", "par"] {
                let file = directory.join(format!("{name}{index}.hevc"));
                let raw = directory.join(format!("{name}{index}.raw"));
                let output = std::process::Command::new(&ffmpeg)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "hevc",
                        "-i",
                        file.to_str().expect("file path"),
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "yuv420p",
                        "-y",
                        raw.to_str().expect("raw path"),
                    ])
                    .output()
                    .expect("ffmpeg runs");
                let pixels = fs::read(&raw).expect("raw frames written");
                let _ = fs::remove_file(&raw);
                let frames = pixels.len() / (WIDTH_CY * HEIGHT_CY * 3 / 2);
                assert!(
                    output.status.success() && frames == *expected_frames,
                    "{name}{index}.hevc decoded {frames} frames, expected {expected_frames}"
                );
                decoded.push(pixels);
            }
            let (worst_delta, differing) = pixel_difference(&decoded[0], &decoded[1]);
            if index == 0 {
                assert_eq!(
                    differing, 0,
                    "the first zone must be pixel-identical to its own encode"
                );
            } else {
                assert!(
                    worst_delta <= MAX_RATE_CONTROL_CARRY_DELTA,
                    "part {index} differs from the independent encode by {worst_delta}, beyond \
                     rate-control carryover"
                );
            }
        }
        let _ = fs::remove_dir_all(&directory);
    }

    const WIDTH_CY: usize = 64;
    const HEIGHT_CY: usize = 64;

    /// Largest per-sample difference tolerated across a zone boundary.
    ///
    /// Only the first zone starts from the encoder's initial state and so
    /// is identical to an independent encode. Every later zone inherits
    /// rate-control history — the lookahead and QP curve carry over the
    /// boundary — so its pixels drift. Measured drift is a small quantizer
    /// step (x264 ~16, x265 ~28 of 255); this bounds it loosely enough to
    /// stay encoder- and content-independent while still failing if a
    /// boundary lands in the wrong place.
    const MAX_RATE_CONTROL_CARRY_DELTA: u8 = 32;

    /// SVT-AV1 splits on IVF frame headers and forces keyframes with
    /// `--force-key-frames` rather than a qpfile. Only a build with WebM IO
    /// (the SVT-AV1-Essential fork) supports `--zones` at all, so this is
    /// skipped unless the installed binary does.
    #[test]
    fn a_real_svt_av1_zone_stream_forces_a_keyframe_at_every_zone_start() {
        let Ok(svt) = which::which("SvtAv1EncApp") else {
            return; // no encoder available to test against
        };
        let directory = temp_directory("svt");
        let source = directory.join("full.y4m");
        // SVT-AV1-Essential is 10-bit only.
        write_test_y4m(&source, 0, 30, 10);

        let scene_frames = [10usize, 10, 10];
        let crfs = [20u32, 40, 30];
        // The config file carries the zones and keyframes; everything else
        // stays on the command line.
        let config = directory.join("zone.config");
        fs::write(
            &config,
            "Zones : 0,9,20;10,19,40;20,29,30\nForceKeyFrames : 10f,20f\n",
        )
        .expect("config written");
        let base = vec![
            "--preset".to_owned(),
            "12".to_owned(),
            "--progress".to_owned(),
            "0".to_owned(),
            "--keyint".to_owned(),
            "0".to_owned(),
            "--scd".to_owned(),
            "0".to_owned(),
            "--rc".to_owned(),
            "0".to_owned(),
        ];

        let mut args: Vec<String> = base;
        args.extend(["-i".to_owned(), "stdin".to_owned()]);
        args.extend([
            "-b".to_owned(),
            directory.join("zone.ivf").display().to_string(),
            "--crf".to_owned(),
            "25".to_owned(),
            // A WebM IO build writes Matroska to a `.ivf` path; this forces
            // the IVF that Av1an splits.
            "--webm".to_owned(),
            "0".to_owned(),
            "-c".to_owned(),
            config.display().to_string(),
        ]);
        let Ok(mut child) = std::process::Command::new(&svt)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return; // the binary exists but cannot be driven this way
        };
        if let Some(stdin) = child.stdin.take() {
            let bytes = fs::read(&source).expect("y4m readable");
            let mut stdin = stdin;
            use std::io::Write as _;
            let _ = stdin.write_all(&bytes);
        }
        let status = child.wait().expect("SvtAv1EncApp runs");
        if !status.success() {
            // Stock SVT-AV1 has no `--zones`; nothing to assert here.
            let _ = fs::remove_dir_all(&directory);
            return;
        }

        let tasks: Vec<Task> = scene_frames
            .iter()
            .enumerate()
            .map(|(index, frames)| Task {
                original_index: index,
                index,
                frame_indices: (0..*frames).collect(),
                sub_scenes: None,
                encoder: svt_av1(crfs[index] as f64),
                output: directory.join(format!("part{index}.ivf")),
            })
            .collect();
        let split = split_ivf(&directory.join("zone.ivf"), &tasks, "ivf").expect("split");
        assert_eq!(
            split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
            scene_frames.to_vec(),
            "each part holds its scene's frames"
        );

        // Every part opens with an IVF header and decodes standalone.
        for task in &tasks {
            let part = fs::read(&task.output).expect("part readable");
            assert!(part.starts_with(b"DKIF"), "part opens with the IVF header");
        }
        if let Ok(ffprobe) = which::which("ffprobe") {
            let file = directory.join("zone.ivf");
            let output = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "frame=pict_type",
                    "-of",
                    "csv=p=0",
                    file.to_str().expect("file path"),
                ])
                .output()
                .expect("ffprobe runs");
            let types: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line.trim().to_owned())
                .collect();
            assert_eq!(
                types.len(),
                30,
                "the whole pass decoded {} frames",
                types.len()
            );
            // A keyframe at every zone start is what makes each part stand
            // in for an independent per-scene encode.
            for zone in [0usize, 10, 20] {
                assert_eq!(
                    types[zone], "I",
                    "frame {zone} should open a zone with a keyframe, found {}",
                    types[zone]
                );
            }
        }
        let _ = fs::remove_dir_all(&directory);
    }

    /// Returns the largest per-sample difference and how many samples differ.
    fn pixel_difference(left: &[u8], right: &[u8]) -> (u8, usize) {
        assert_eq!(left.len(), right.len(), "compared frames are the same size");
        let mut worst = 0u8;
        let mut differing = 0usize;
        for (a, b) in left.iter().zip(right.iter()) {
            let delta = a.abs_diff(*b);
            if delta != 0 {
                differing += 1;
                worst = worst.max(delta);
            }
        }
        (worst, differing)
    }

    /// Writes `count` Y4M frames of a distinct per-frame gradient, starting at
    /// source position `start`. `bit_depth` 8 writes `C420`, 10 writes
    /// `C420p10` with two bytes per sample.
    fn write_test_y4m(path: &Path, start: usize, count: usize, bit_depth: u8) {
        const WIDTH: usize = WIDTH_CY;
        const HEIGHT: usize = HEIGHT_CY;
        let sample_bytes = usize::from(bit_depth > 8) + 1;
        let mut bytes = Vec::new();
        let chroma = if bit_depth == 8 {
            "C420".to_owned()
        } else {
            format!("C420p{bit_depth}")
        };
        bytes.extend_from_slice(
            format!("YUV4MPEG2 W{WIDTH} H{HEIGHT} F30:1 Ip A1:1 {chroma}\n").as_bytes(),
        );
        for index in start..start + count {
            bytes.extend_from_slice(b"FRAME\n");
            for offset in 0..HEIGHT {
                let mut row = vec![0u8; WIDTH * sample_bytes];
                // A diagonal ramp differs in every frame, so a wrong boundary
                // shows up as a visibly different picture.
                row[0] = ((index * 4 + offset) % 256) as u8;
                row[WIDTH * sample_bytes / 2] = ((index * 7) % 256) as u8;
                bytes.extend_from_slice(&row);
            }
            let plane = (WIDTH / 2) * (HEIGHT / 2) * sample_bytes;
            bytes.extend(std::iter::repeat_n((index % 256) as u8, plane * 2));
        }
        fs::write(path, bytes).expect("y4m written");
    }

    #[test]
    fn nal_reader_reassembles_every_segment() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2]);
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x21, 0xa0]);
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a, 0x22]);
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x41, 0x11]);
        stream.extend_from_slice(&[0x00, 0x00, 0x00]); // trailing zeros

        let mut reader = NalReader::new(Cursor::new(stream.clone()));
        let mut segments = Vec::new();
        while let Some(segment) = reader.next_segment().expect("readable") {
            segments.push(segment);
        }
        let reassembled: Vec<u8> = segments.iter().flatten().copied().collect();
        assert_eq!(reassembled, stream);
        // One segment per start code: the orphan leading zero rides with the
        // first, and the trailing zeros ride with the last.
        assert_eq!(segments.len(), 5);
        // The orphan leading zero stays with the first segment.
        assert_eq!(segments[0], vec![
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f
        ]);
        assert!(segments[2].starts_with(&[0x00, 0x00, 0x01, 0x65]));
    }

    #[test]
    fn nal_reader_finds_start_codes_split_across_reads() {
        let stream: Vec<u8> =
            vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x00, 0x00, 0x00, 0x01, 0x41, 0x80];
        // One byte at a time so every boundary splits the stream awkwardly.
        struct Trickle<R> {
            inner: R,
        }
        impl Read for Trickle<Cursor<Vec<u8>>> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let limit = 1.min(buffer.len());
                self.inner.read(&mut buffer[..limit])
            }
        }
        impl BufRead for Trickle<Cursor<Vec<u8>>> {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                let buffer = self.inner.fill_buf()?;
                // One byte per read, so every start code straddles a boundary.
                Ok(&buffer[..buffer.len().min(1)])
            }
            fn consume(&mut self, amount: usize) {
                self.inner.consume(amount);
            }
        }
        let mut reader = NalReader::new(Trickle {
            inner: Cursor::new(stream.clone()),
        });
        let mut segments = Vec::new();
        while let Some(segment) = reader.next_segment().expect("readable") {
            segments.push(segment);
        }
        let reassembled: Vec<u8> = segments.iter().flatten().copied().collect();
        assert_eq!(reassembled, stream);
    }

    /// A whole-pass IVF file, with the frame count the encoder writes into the
    /// header covering every frame of the pass.
    fn synthetic_ivf(frame_sizes: &[u32]) -> Vec<u8> {
        let mut ivf = vec![0u8; 32];
        ivf[..4].copy_from_slice(b"DKIF");
        ivf[IVF_NFRAME_OFFSET..IVF_NFRAME_OFFSET + 4]
            .copy_from_slice(&(frame_sizes.len() as u32).to_le_bytes());
        for (index, size) in frame_sizes.iter().enumerate() {
            ivf.extend_from_slice(&size.to_le_bytes());
            ivf.extend_from_slice(&(index as u64).to_le_bytes());
            ivf.extend(std::iter::repeat_n(0xAB, *size as usize));
        }
        ivf
    }

    /// The frame count a file's IVF header declares.
    fn ivf_nframe(path: &Path) -> u32 {
        u32::from_le_bytes(
            fs::read(path).expect("part read")[IVF_NFRAME_OFFSET..][..4]
                .try_into()
                .expect("4 bytes"),
        )
    }

    #[test]
    fn ivf_output_splits_into_per_scene_files() {
        let directory = temp_directory("ivf");
        let whole = directory.join("zone-pass.temp.ivf");
        // Frames: scene 0 gets 2, scene 1 gets 3, scene 2 gets 1.
        fs::write(&whole, synthetic_ivf(&[3, 5, 7, 2, 9, 4])).expect("ivf written");
        let tasks = vec![
            Task {
                original_index: 0,
                index:          0,
                frame_indices:  (0..2).collect(),
                sub_scenes:     None,
                encoder:        x264(20.0),
                output:         directory.join("scene0.ivf"),
            },
            Task {
                original_index: 1,
                index:          1,
                frame_indices:  (0..3).collect(),
                sub_scenes:     None,
                encoder:        x264(30.0),
                output:         directory.join("scene1.ivf"),
            },
            Task {
                original_index: 2,
                index:          2,
                frame_indices:  (0..1).collect(),
                sub_scenes:     None,
                encoder:        x264(40.0),
                output:         directory.join("scene2.ivf"),
            },
        ];
        let split = split_ivf(&whole, &tasks, "ivf").expect("split");
        // 32-byte header + frame headers + payloads.
        assert_eq!(split, vec![
            (32 + 12 + 3 + 12 + 5, 2),
            (32 + 12 + 7 + 12 + 2 + 12 + 9, 3),
            (32 + 12 + 4, 1)
        ]);
        for task in &tasks {
            assert!(task.output.exists(), "{} saved", task.output.display());
        }
        // Each part starts with the IVF header.
        let head = fs::read(&tasks[1].output).expect("scene1 read");
        assert_eq!(&head[..4], b"DKIF");
        assert_eq!(head.len() as u64, split[1].0);
        // Each part declares its own frame count, not the whole pass's: the
        // scene concatenator sums these headers to rebuild the duration.
        assert_eq!(
            tasks.iter().map(|task| ivf_nframe(&task.output)).collect::<Vec<_>>(),
            vec![2, 3, 1],
            "per-scene frame counts"
        );
        let _ = fs::remove_file(&whole);
        let _ = fs::remove_dir_all(&directory);
    }

    fn synthetic_h264(frames: usize) -> Vec<u8> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2]);
        for frame in 0..frames {
            let nal_type = if frame % 2 == 0 { 0x65 } else { 0x41 };
            stream.extend_from_slice(&[0x00, 0x00, 0x01, nal_type, 0x88, 0xC0]);
            stream.extend(std::iter::repeat_n(frame as u8 + 1, 4));
        }
        stream
    }

    #[test]
    fn annexb_output_splits_with_headers_in_every_file() {
        let directory = temp_directory("annexb");
        let whole = directory.join("zone-pass.temp.264");
        fs::write(&whole, synthetic_h264(6)).expect("stream written");
        let tasks = vec![
            Task {
                original_index: 0,
                index:          0,
                frame_indices:  (0..2).collect(),
                sub_scenes:     None,
                encoder:        x264(20.0),
                output:         directory.join("scene0.264"),
            },
            Task {
                original_index: 1,
                index:          1,
                frame_indices:  (0..3).collect(),
                sub_scenes:     None,
                encoder:        x264(30.0),
                output:         directory.join("scene1.264"),
            },
            Task {
                original_index: 2,
                index:          2,
                frame_indices:  (0..1).collect(),
                sub_scenes:     None,
                encoder:        x264(40.0),
                output:         directory.join("scene2.264"),
            },
        ];
        let split = split_annexb(&whole, Codec::H264, &tasks, "264").expect("split");
        assert_eq!(
            split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        let prefix: Vec<u8> = synthetic_h264(6)[..15].to_vec(); // SPS + PPS
        // Every file opens with the headers, so each decodes standalone.
        for task in &tasks {
            let bytes = fs::read(&task.output).expect("part read");
            assert!(
                bytes.starts_with(&prefix),
                "{} lacks the stream headers",
                task.output.display()
            );
        }
        // Stripping the repeated headers reconstructs the original stream.
        let mut reassembled = fs::read(&tasks[0].output).expect("part 0");
        for task in tasks.iter().skip(1) {
            reassembled.extend_from_slice(&fs::read(&task.output).expect("part")[prefix.len()..]);
        }
        assert_eq!(reassembled, fs::read(&whole).expect("whole"));
        let _ = fs::remove_file(&whole);
        let _ = fs::remove_dir_all(&directory);
    }
}
