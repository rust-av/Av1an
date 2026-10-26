use std::{collections::HashMap, path::PathBuf};

use andean_condor::models::{
    encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter, photon_noise::PhotonNoise},
    scene::Scene,
};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    configuration::CliSequenceData,
    scenes::sub_scenes_within,
    utils::parameter_parser::EncoderParamsParser,
};

/// A frame range whose scenes are overridden.
///
/// Frames are half-open (`start_frame` inclusive, `end_frame` exclusive) and
/// resolve against the input: `None` starts at the first frame or ends at the
/// last one. Every scene sharing at least one frame with the range is modified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Zone {
    /// First frame of the zone (inclusive). `None` is the input's first frame.
    #[serde(default)]
    pub start_frame:    Option<usize>,
    /// End of the zone (exclusive). `None` is the input's last frame.
    #[serde(default)]
    pub end_frame:      Option<usize>,
    /// Splits scenes longer than this, so no scene a zone matches exceeds it.
    #[serde(default)]
    pub maximum_frames: Option<usize>,
    /// The encoder override applied to the zone's scenes. `None` only splits.
    #[serde(default)]
    pub encoder:        Option<ZoneEncoder>,
}

impl Zone {
    /// The zone's half-open frame range for an input of `num_frames`.
    #[inline]
    pub fn resolve(&self, num_frames: usize) -> (usize, usize) {
        (
            self.start_frame.unwrap_or(0),
            self.end_frame.unwrap_or(num_frames),
        )
    }

    /// Whether the zone shares a frame with the half-open range `start..end`.
    #[inline]
    pub fn intersects(&self, start: usize, end: usize, num_frames: usize) -> bool {
        let (zone_start, zone_end) = self.resolve(num_frames);
        zone_start < end && start < zone_end
    }

    /// Checks the zone against an input of `num_frames`.
    pub fn validate(&self, num_frames: usize) -> Result<()> {
        let (start, end) = self.resolve(num_frames);
        if start >= num_frames {
            bail!("start frame {start} is past the input's {num_frames} frames");
        }
        if end > num_frames {
            bail!("end frame {end} is past the input's {num_frames} frames");
        }
        if start >= end {
            bail!("start frame {start} must come before end frame {end}");
        }
        if self.maximum_frames == Some(0) {
            bail!("maximum_frames must be at least 1");
        }
        if let Some(ZoneEncoder::Merge(merge)) = &self.encoder
            && merge.base == Some(EncoderBase::FFmpeg)
            && merge.ffmpeg_filter.is_some()
        {
            bail!(
                "ffmpeg_filter is not supported by the FFmpeg encoder; use native FFmpeg options \
                 instead"
            );
        }

        Ok(())
    }

    /// Applies the zone to `scene`, splitting it when the zone caps its length.
    /// Returns the scene as it came in, with a warning, when the zone cannot
    /// change it.
    pub fn apply(
        &self,
        index: usize,
        mut scene: Scene<CliSequenceData>,
        warnings: &mut Vec<anyhow::Error>,
    ) -> Vec<Scene<CliSequenceData>> {
        if let Some(encoder) = &self.encoder {
            match encoder {
                ZoneEncoder::Replace(replacement) => scene.encoder.clone_from(replacement),
                ZoneEncoder::Merge(merge) => merge.apply(index, &mut scene, warnings),
            }
        }

        let Some(maximum_frames) = self.maximum_frames.filter(|frames| *frames > 0) else {
            return vec![scene];
        };
        if scene.end_frame - scene.start_frame <= maximum_frames {
            return vec![scene];
        }
        split(&scene, maximum_frames)
    }
}

/// Names a zone in warnings and errors.
#[inline]
pub(crate) fn zone_label(index: usize, start: usize, end: usize) -> String {
    format!("zone {} ({start}..{end})", index + 1)
}

/// How a zone's encoder change is applied to a scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ZoneEncoder {
    /// Replaces the scene's encoder with this one entirely, dropping the
    /// scene's executable, options, Photon Noise and FFmpeg filter.
    Replace(Encoder),
    /// Merges the given settings into the scene's encoder.
    Merge(ZoneEncoderMerge),
}

/// The encoder settings a merge zone contributes. Every field is optional and
/// overrides the scene's own.
///
/// Zone strings always merge options and Photon Noise; JSON can select their
/// replacement modes independently.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZoneEncoderMerge {
    /// The encoder base the zone's settings belong to. Scenes with another base
    /// are left unchanged, which keeps one encoder's parameters off another's
    /// command line.
    #[serde(default)]
    pub base:              Option<EncoderBase>,
    /// The encoder binary to use instead of the base's default.
    #[serde(default)]
    pub executable:        Option<PathBuf>,
    /// The encoder options to merge into (or replace) the scene's.
    #[serde(default)]
    pub options:           Option<ZoneOptions>,
    /// The Photon Noise table to merge into (or replace) the scene's.
    #[serde(default)]
    pub photon_noise:      Option<ZonePhotonNoise>,
    /// The FFmpeg filtergraph to apply to the scene's frames.
    #[serde(default)]
    pub ffmpeg_filter:     Option<String>,
    /// Whether to append the source's colour and HDR metadata to the scene's
    /// encoder parameters.
    #[serde(default)]
    pub append_color_data: Option<bool>,
}

impl ZoneEncoderMerge {
    /// Whether the merge sets no encoder setting; a base alone only states an
    /// expectation.
    #[inline]
    fn is_empty(&self) -> bool {
        self.executable.is_none()
            && self.options.is_none()
            && self.photon_noise.is_none()
            && self.ffmpeg_filter.is_none()
            && self.append_color_data.is_none()
    }

    fn apply(
        &self,
        index: usize,
        scene: &mut Scene<CliSequenceData>,
        warnings: &mut Vec<anyhow::Error>,
    ) {
        if self.is_empty() {
            return;
        }
        if let Some(base) = self.base
            && base != scene.encoder.base()
        {
            warnings.push(anyhow::anyhow!(
                "{} merges {base} settings into scene {}..{}, which uses {}; leaving it unchanged",
                zone_label(index, scene.start_frame, scene.end_frame),
                scene.start_frame,
                scene.end_frame,
                scene.encoder.base()
            ));
            return;
        }

        if let Some(executable) = &self.executable {
            scene.encoder.set_executable(Some(executable.clone()));
        }
        if let Some(options) = &self.options {
            match options.parameters(scene.encoder.base()) {
                Ok(parameters) => match options {
                    ZoneOptions::Merge(_) => scene.encoder.parameters_mut().extend(parameters),
                    ZoneOptions::Replace(_) => *scene.encoder.parameters_mut() = parameters,
                },
                Err(error) => warnings.push(anyhow::anyhow!(
                    "{} could not read its encoder options: {error}",
                    zone_label(index, scene.start_frame, scene.end_frame)
                )),
            }
        }
        if let Some(zone_noise) = &self.photon_noise {
            apply_photon_noise(index, scene, zone_noise, warnings);
        }
        if let Some(append_color_data) = self.append_color_data {
            scene.encoder.set_append_color_data(append_color_data);
        }
        if let Some(filter) = &self.ffmpeg_filter {
            if scene.encoder.base() == EncoderBase::FFmpeg {
                warnings.push(anyhow::anyhow!(
                    "scene {}..{} uses FFmpeg, which does not support ffmpeg_filter; use native \
                     FFmpeg options instead",
                    scene.start_frame,
                    scene.end_frame
                ));
            } else {
                scene.encoder.set_ffmpeg_filter(Some(filter.clone()));
            }
        }
    }
}

/// How a zone's encoder options are applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ZoneOptions {
    /// Merges these parameters over the scene's.
    Merge(ZoneOptionsBody),
    /// Replaces the scene's parameters with these, seeded from the encoder
    /// base's defaults.
    Replace(ZoneOptionsBody),
}

impl ZoneOptions {
    /// The parameters to apply for `base`.
    fn parameters(&self, base: EncoderBase) -> Result<HashMap<String, CLIParameter>> {
        let parameters = match self {
            ZoneOptions::Merge(body) => body.parameters()?,
            ZoneOptions::Replace(body) => {
                let mut parameters = base.default_parameters();
                parameters.extend(body.parameters()?);
                parameters
            },
        };

        Ok(parameters)
    }
}

/// Encoder parameters written either as a `--params` style string or as parsed
/// parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ZoneOptionsBody {
    /// A parameter string, parsed like *--params*.
    Params(String),
    /// Parsed encoder parameters.
    Map(HashMap<String, CLIParameter>),
}

impl ZoneOptionsBody {
    /// The parsed parameters.
    #[inline]
    pub fn parameters(&self) -> Result<HashMap<String, CLIParameter>> {
        match self {
            Self::Params(parameters) => Ok(EncoderParamsParser::parse_string(parameters)?),
            Self::Map(parameters) => Ok(parameters.clone()),
        }
    }
}

/// How a zone's Photon Noise change is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ZonePhotonNoise {
    /// Patches the scene's table field by field, keeping the fields the
    /// override does not name.
    Merge(ZonePhotonNoiseBody),
    /// Replaces the scene's table with this one entirely, dropping the fields
    /// the override does not name.
    Replace(ZonePhotonNoiseBody),
}

/// A Photon Noise table, or the fields a scene's table is patched with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZonePhotonNoiseBody {
    #[serde(default)]
    pub iso:        Option<u32>,
    #[serde(default)]
    pub chroma_iso: Option<u32>,
    #[serde(default)]
    pub width:      Option<u32>,
    #[serde(default)]
    pub height:     Option<u32>,
    #[serde(default)]
    pub c_y:        Option<Vec<i8>>,
    #[serde(default)]
    pub ccb:        Option<Vec<i8>>,
    #[serde(default)]
    pub ccr:        Option<Vec<i8>>,
}

impl ZonePhotonNoiseBody {
    /// Whether the override names no field at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The whole table, which needs an ISO.
    pub fn table(&self) -> Result<PhotonNoise> {
        let Some(iso) = self.iso else {
            bail!("a Photon Noise table needs an iso");
        };

        Ok(PhotonNoise {
            iso,
            chroma_iso: self.chroma_iso,
            width: self.width,
            height: self.height,
            c_y: self.c_y.clone(),
            ccb: self.ccb.clone(),
            ccr: self.ccr.clone(),
        })
    }

    /// `existing` with the override's fields applied, or a new table when the
    /// override carries an ISO and the scene has none.
    pub fn merged(&self, existing: Option<&PhotonNoise>) -> Result<Option<PhotonNoise>> {
        if self.is_empty() {
            return Ok(existing.cloned());
        }
        let Some(iso) = self.iso.or_else(|| existing.map(|table| table.iso)) else {
            bail!("a Photon Noise merge needs an iso when the scene has no table");
        };

        Ok(Some(PhotonNoise {
            iso,
            chroma_iso: self.chroma_iso.or_else(|| existing.and_then(|table| table.chroma_iso)),
            width: self.width.or_else(|| existing.and_then(|table| table.width)),
            height: self.height.or_else(|| existing.and_then(|table| table.height)),
            c_y: self.c_y.clone().or_else(|| existing.and_then(|table| table.c_y.clone())),
            ccb: self.ccb.clone().or_else(|| existing.and_then(|table| table.ccb.clone())),
            ccr: self.ccr.clone().or_else(|| existing.and_then(|table| table.ccr.clone())),
        }))
    }
}

/// Applies a zone's Photon Noise to `scene`, warning when the scene's encoder
/// has no table or the override cannot be read.
fn apply_photon_noise(
    index: usize,
    scene: &mut Scene<CliSequenceData>,
    zone_noise: &ZonePhotonNoise,
    warnings: &mut Vec<anyhow::Error>,
) {
    if !scene.encoder.base().supports_photon_noise() {
        warnings.push(anyhow::anyhow!(
            "{} sets Photon Noise but scene {}..{} uses {}, which has none; leaving it unchanged",
            zone_label(index, scene.start_frame, scene.end_frame),
            scene.start_frame,
            scene.end_frame,
            scene.encoder.base()
        ));

        return;
    }

    let existing = scene.encoder.photon_noise().cloned();
    let applied = match zone_noise {
        ZonePhotonNoise::Merge(body) => body.merged(existing.as_ref()),
        ZonePhotonNoise::Replace(body) => body.table().map(Some),
    };
    match applied {
        Ok(photon_noise) => scene.encoder.set_photon_noise(photon_noise),
        Err(error) => warnings.push(anyhow::anyhow!(
            "{} could not apply Photon Noise to scene {}..{}: {error}",
            zone_label(index, scene.start_frame, scene.end_frame),
            scene.start_frame,
            scene.end_frame
        )),
    }
}

/// Splits a scene into equal pieces of at most `maximum_frames`. Every frame of
/// the scene lands in exactly one piece.
fn split(scene: &Scene<CliSequenceData>, maximum_frames: usize) -> Vec<Scene<CliSequenceData>> {
    let frames = scene.end_frame - scene.start_frame;
    let pieces = frames.div_ceil(maximum_frames);

    (0..pieces)
        .map(|piece| {
            let start = scene.start_frame + piece * frames / pieces;
            let end = scene.start_frame + (piece + 1) * frames / pieces;
            Scene {
                start_frame:   start,
                end_frame:     end,
                sub_scenes:    sub_scenes_within(scene.sub_scenes.as_deref(), start, end),
                encoder:       scene.encoder.clone(),
                sequence_data: scene.sequence_data.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{encoder::EncoderPasses, scene::SubScene};

    use super::*;

    fn svt_av1() -> Encoder {
        Encoder::default_from_base(&EncoderBase::SVTAV1, false)
    }

    fn scene(start_frame: usize, end_frame: usize) -> Scene<CliSequenceData> {
        Scene {
            start_frame,
            end_frame,
            sub_scenes: None,
            encoder: Encoder::default_from_base(&EncoderBase::SVTAV1, false),
            sequence_data: CliSequenceData::default(),
        }
    }

    fn parameters(values: &[(&str, f64)]) -> HashMap<String, CLIParameter> {
        CLIParameter::new_numbers("--", " ", values)
    }

    fn zone_with(encoder: Option<ZoneEncoder>) -> Zone {
        Zone {
            start_frame: Some(0),
            end_frame: Some(100),
            maximum_frames: None,
            encoder,
        }
    }

    fn merge(merge: ZoneEncoderMerge) -> Zone {
        zone_with(Some(ZoneEncoder::Merge(merge)))
    }

    fn applied(zone: &Zone, scene: Scene<CliSequenceData>) -> Vec<anyhow::Error> {
        let mut warnings = Vec::new();
        let _ = zone.apply(0, scene, &mut warnings);

        warnings
    }

    fn photon_noise(iso: u32) -> PhotonNoise {
        PhotonNoise {
            iso,
            chroma_iso: Some(iso / 3),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }
    }

    #[test]
    fn a_replace_zone_swaps_the_whole_encoder() {
        let mut replacement = Encoder::default_from_base(&EncoderBase::X265, false);
        replacement.set_executable(Some(PathBuf::from("/tools/x265")));
        replacement.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        let zone = zone_with(Some(ZoneEncoder::Replace(replacement.clone())));
        let mut scene = scene(0, 100);
        scene.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);

        let mut warnings = Vec::new();
        let scenes = zone.apply(0, scene, &mut warnings);

        assert!(warnings.is_empty(), "zone applies cleanly");
        assert_eq!(scenes.len(), 1);
        assert_eq!(
            scenes[0].encoder, replacement,
            "encoder is entirely replaced"
        );
    }

    #[test]
    fn a_merge_zone_patches_options_over_the_scene() {
        let mut scene = scene(0, 100);
        scene.encoder.parameters_mut().insert(
            "aq-mode".to_owned(),
            CLIParameter::new_number("--", " ", 2.0),
        );
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            options: Some(ZoneOptions::Merge(ZoneOptionsBody::Map(parameters(&[
                ("crf", 23.0),
                ("preset", 3.0),
            ])))),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene.clone());

        // The scene's own parameters and the base's defaults survive a merge.
        let mut expected = scene.encoder.clone();
        expected
            .parameters_mut()
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 23.0));
        expected.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 3.0),
        );
        assert_eq!(encoder, expected);
    }

    /// The encoder a zone produces for a scene.
    fn merge_encoder(zone: &Zone, scene: Scene<CliSequenceData>) -> Encoder {
        let mut warnings = Vec::new();
        zone.apply(0, scene, &mut warnings)
            .into_iter()
            .next()
            .expect("zone returns a scene")
            .encoder
    }

    #[test]
    fn replacing_options_seeds_the_base_defaults() {
        let scene = scene(0, 100);
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            options: Some(ZoneOptions::Replace(ZoneOptionsBody::Map(parameters(&[(
                "preset", 3.0,
            )])))),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene);

        let expected = parameters(&[
            ("preset", 3.0),
            ("keyint", 0.0),
            ("scd", 0.0),
            ("rc", 0.0),
            ("crf", 25.0),
            ("progress", 2.0),
        ]);
        assert_eq!(encoder.parameters(), &expected);
        // Unaffected by the option replacement.
        assert_eq!(encoder.passes(), EncoderPasses::All(1));
    }

    #[test]
    fn a_parameter_string_parses_like_the_params_flag() {
        let scene = scene(0, 100);
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            options: Some(ZoneOptions::Merge(ZoneOptionsBody::Params(
                "--preset 3 --crf 23".to_owned(),
            ))),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene);

        assert_eq!(
            encoder.parameters().get("preset"),
            Some(&CLIParameter::new_number("--", " ", 3.0))
        );
        assert_eq!(
            encoder.parameters().get("crf"),
            Some(&CLIParameter::new_number("--", " ", 23.0))
        );
    }

    #[test]
    fn a_merge_zone_patches_executable_and_filter() {
        let mut scene = scene(0, 100);
        scene.encoder.set_photon_noise(Some(photon_noise(400)));
        scene.encoder.parameters_mut().insert(
            "film-grain".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        // `Encoder::executable` falls back to the base's default name for a path
        // that does not exist, so the zone's binary has to be a real file.
        let binary = std::env::current_exe().expect("the test binary has a path");
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            executable: Some(binary.clone()),
            ffmpeg_filter: Some("eq=brightness=0.05".to_owned()),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene);

        assert_eq!(encoder.executable(), binary.to_str().expect("a UTF-8 path"));
        assert_eq!(encoder.ffmpeg_filter(), Some("eq=brightness=0.05"));
        // Untouched settings stay.
        assert_eq!(encoder.photon_noise(), Some(&photon_noise(400)));
        assert!(encoder.parameters().contains_key("film-grain"));
    }

    #[test]
    fn a_photon_noise_merge_keeps_unset_fields() {
        let mut scene = scene(0, 100);
        let mut existing = photon_noise(400);
        existing.c_y = Some(vec![34, -1, 32]);
        scene.encoder.set_photon_noise(Some(existing));
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            photon_noise: Some(ZonePhotonNoise::Merge(ZonePhotonNoiseBody {
                chroma_iso: Some(800),
                ..Default::default()
            })),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene);

        let table = encoder.photon_noise().expect("photon noise survives");
        assert_eq!(table.iso, 400, "unset iso keeps the scene's");
        assert_eq!(table.chroma_iso, Some(800), "set iso overrides the scene's");
        assert_eq!(
            table.c_y,
            Some(vec![34, -1, 32]),
            "unset tables keep the scene's"
        );
    }

    #[test]
    fn a_photon_noise_replacement_drops_the_scene_fields() {
        let mut scene = scene(0, 100);
        let mut existing = photon_noise(400);
        existing.c_y = Some(vec![34, -1, 32]);
        scene.encoder.set_photon_noise(Some(existing));
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            photon_noise: Some(ZonePhotonNoise::Replace(ZonePhotonNoiseBody {
                iso: Some(800),
                ..Default::default()
            })),
            ..Default::default()
        });

        let encoder = merge_encoder(&zone, scene);

        assert_eq!(
            encoder.photon_noise(),
            Some(&PhotonNoise {
                iso:        800,
                chroma_iso: None,
                width:      None,
                height:     None,
                c_y:        None,
                ccb:        None,
                ccr:        None,
            }),
            "a replacement keeps none of the scene's table"
        );
    }

    #[test]
    fn a_photon_noise_replacement_without_an_iso_warns() {
        let mut scene = scene(0, 100);
        let existing = photon_noise(400);
        scene.encoder.set_photon_noise(Some(existing.clone()));
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            photon_noise: Some(ZonePhotonNoise::Replace(ZonePhotonNoiseBody {
                chroma_iso: Some(800),
                ..Default::default()
            })),
            ..Default::default()
        });

        let warnings = applied(&zone, scene.clone());

        assert_eq!(warnings.len(), 1, "the impossible replacement is reported");
        assert!(
            warnings[0].to_string().contains("needs an iso"),
            "warning explains the missing iso: {}",
            warnings[0]
        );
        assert_eq!(
            merge_encoder(&zone, scene).photon_noise(),
            Some(&existing),
            "the scene keeps its table"
        );
    }

    #[test]
    fn a_photon_noise_merge_without_a_table_or_iso_warns() {
        let scene = scene(0, 100);
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::SVTAV1),
            photon_noise: Some(ZonePhotonNoise::Merge(ZonePhotonNoiseBody {
                chroma_iso: Some(800),
                ..Default::default()
            })),
            ..Default::default()
        });

        let warnings = applied(&zone, scene.clone());

        assert_eq!(warnings.len(), 1, "the impossible merge is reported");
        assert!(
            warnings[0].to_string().contains("needs an iso"),
            "warning explains the missing iso: {}",
            warnings[0]
        );
        assert_eq!(
            merge_encoder(&zone, scene),
            svt_av1(),
            "the scene is left unchanged"
        );
    }

    #[test]
    fn a_photon_noise_merge_onto_an_encoder_without_one_warns() {
        let mut scene = scene(0, 100);
        scene.encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::X264),
            photon_noise: Some(ZonePhotonNoise::Merge(ZonePhotonNoiseBody {
                iso: Some(400),
                ..Default::default()
            })),
            ..Default::default()
        });

        let warnings = applied(&zone, scene);

        assert_eq!(warnings.len(), 1, "the unsupported base is reported");
        assert!(
            warnings[0].to_string().contains("has none"),
            "warning names the missing table: {}",
            warnings[0]
        );
    }

    #[test]
    fn a_merge_zone_for_another_base_warns_and_changes_nothing() {
        let scene = scene(0, 100);
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::X264),
            options: Some(ZoneOptions::Merge(ZoneOptionsBody::Map(parameters(&[(
                "preset", 3.0,
            )])))),
            ..Default::default()
        });

        let warnings = applied(&zone, scene.clone());

        assert_eq!(warnings.len(), 1, "the base mismatch is reported");
        assert!(
            warnings[0].to_string().contains("leaving it unchanged"),
            "warning says the scene is skipped: {}",
            warnings[0]
        );
        assert_eq!(merge_encoder(&zone, scene), svt_av1());
    }

    #[test]
    fn maximum_frames_splits_a_scene_evenly() {
        let mut scene = scene(0, 720);
        scene.sub_scenes = Some(vec![
            SubScene {
                start_frame: 10,
                end_frame:   20,
            },
            SubScene {
                start_frame: 400,
                end_frame:   420,
            },
        ]);
        let zone = Zone {
            maximum_frames: Some(100),
            ..zone_with(None)
        };

        let mut warnings = Vec::new();
        let scenes = zone.apply(0, scene, &mut warnings);

        assert!(warnings.is_empty(), "splitting warns about nothing");
        assert_eq!(scenes.len(), 8, "720 frames of 90 cover the original scene");
        assert_eq!(
            scenes.first().expect("a first piece").start_frame,
            0,
            "the first piece starts the scene"
        );
        assert_eq!(
            scenes.last().expect("a last piece").end_frame,
            720,
            "the last piece ends the scene"
        );
        for (index, piece) in scenes.iter().enumerate() {
            assert!(
                piece.end_frame - piece.start_frame <= 100,
                "piece {index} is at most 100 frames"
            );
            assert_eq!(piece.start_frame, index * 90, "pieces are contiguous");
            assert_eq!(
                piece.encoder, scenes[0].encoder,
                "every piece keeps the encoder"
            );
        }
        assert_eq!(
            scenes[0].sub_scenes,
            Some(vec![SubScene {
                start_frame: 10,
                end_frame:   20,
            }]),
            "a sub-scene stays with the piece it starts in"
        );
        assert_eq!(
            scenes[4].sub_scenes,
            Some(vec![SubScene {
                start_frame: 400,
                end_frame:   420,
            }]),
            "a later sub-scene moves to its own piece"
        );
        assert_eq!(
            scenes[1].sub_scenes, None,
            "pieces without sub-scenes keep none"
        );
    }

    #[test]
    fn a_scene_within_the_maximum_is_not_split() {
        let zone = Zone {
            maximum_frames: Some(100),
            ..zone_with(None)
        };

        let mut warnings = Vec::new();
        let scenes = zone.apply(0, scene(0, 100), &mut warnings);

        assert_eq!(scenes.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_zone_without_an_encoder_only_splits() {
        let zone = Zone {
            maximum_frames: Some(50),
            ..zone_with(None)
        };

        let mut warnings = Vec::new();
        let scenes = zone.apply(0, scene(0, 100), &mut warnings);

        assert_eq!(scenes.len(), 2);
        assert_eq!(scenes[0].encoder, svt_av1(), "the encoder is untouched");
    }

    #[test]
    fn validation_rejects_an_empty_range() {
        let zone = Zone {
            start_frame: Some(100),
            end_frame: Some(100),
            ..zone_with(None)
        };

        let error = zone.validate(1000).expect_err("an empty zone is invalid");
        assert!(
            error.to_string().contains("must come before"),
            "error explains the range: {error}"
        );
    }

    #[test]
    fn validation_resolves_unset_bounds_and_rejects_the_past() {
        let zone = Zone {
            start_frame: None,
            end_frame: None,
            ..zone_with(None)
        };
        assert!(
            zone.validate(300).is_ok(),
            "an unbounded zone covers the whole input"
        );

        let past = Zone {
            start_frame: Some(300),
            end_frame: None,
            ..zone_with(None)
        };
        let error = past.validate(300).expect_err("a zone past the input is invalid");
        assert!(
            error.to_string().contains("past the input"),
            "error explains the input length: {error}"
        );
    }

    #[test]
    fn validation_accepts_the_input_end_but_rejects_frames_beyond_it() {
        let zone = Zone {
            end_frame: Some(100),
            ..zone_with(None)
        };
        assert!(zone.validate(100).is_ok());
        let past = Zone {
            end_frame: Some(101),
            ..zone
        };
        assert!(
            past.validate(100)
                .expect_err("end is beyond input")
                .to_string()
                .contains("end frame 101 is past")
        );
    }

    #[test]
    fn splits_clip_crossing_sub_scenes_to_each_parent() {
        let mut source = scene(0, 100);
        source.sub_scenes = Some(vec![
            SubScene {
                start_frame: 40,
                end_frame:   60,
            },
            SubScene {
                start_frame: 50,
                end_frame:   50,
            },
        ]);
        let zone = Zone {
            maximum_frames: Some(50),
            ..zone_with(None)
        };
        let pieces = zone.apply(0, source, &mut Vec::new());
        assert_eq!(
            pieces[0].sub_scenes,
            Some(vec![SubScene {
                start_frame: 40,
                end_frame:   50,
            }])
        );
        assert_eq!(
            pieces[1].sub_scenes,
            Some(vec![SubScene {
                start_frame: 50,
                end_frame:   60,
            }])
        );
    }

    #[test]
    fn an_untyped_filter_merge_warns_on_ffmpeg_instead_of_disappearing() {
        let mut source = scene(0, 100);
        source.encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
        let zone = merge(ZoneEncoderMerge {
            ffmpeg_filter: Some("hflip".to_owned()),
            ..Default::default()
        });
        let warnings = applied(&zone, source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].to_string().contains("does not support ffmpeg_filter"));
    }

    #[test]
    fn validation_rejects_an_explicit_ffmpeg_filter_merge() {
        let zone = merge(ZoneEncoderMerge {
            base: Some(EncoderBase::FFmpeg),
            ffmpeg_filter: Some("hflip".to_owned()),
            ..Default::default()
        });
        assert!(
            zone.validate(100)
                .expect_err("unsupported filter")
                .to_string()
                .contains("not supported")
        );
    }

    #[test]
    fn validation_rejects_a_zero_maximum() {
        let zone = Zone {
            maximum_frames: Some(0),
            ..zone_with(None)
        };

        let error = zone.validate(1000).expect_err("a zero maximum is invalid");
        assert!(
            error.to_string().contains("at least 1"),
            "error explains the minimum: {error}"
        );
    }

    #[test]
    fn normalization_round_trips_through_json() {
        let zone = Zone {
            start_frame:    Some(0),
            end_frame:      Some(100),
            maximum_frames: Some(100),
            encoder:        Some(ZoneEncoder::Merge(ZoneEncoderMerge {
                base:              Some(EncoderBase::SVTAV1),
                executable:        Some(PathBuf::from("/tools/fish")),
                options:           Some(ZoneOptions::Merge(ZoneOptionsBody::Params(
                    "--preset 3 --crf 23".to_owned(),
                ))),
                photon_noise:      Some(ZonePhotonNoise::Replace(ZonePhotonNoiseBody {
                    iso: Some(400),
                    chroma_iso: Some(800),
                    c_y: Some(vec![34, -1, 32]),
                    ..Default::default()
                })),
                ffmpeg_filter:     Some("crop=iw-16:ih-16".to_owned()),
                append_color_data: None,
            })),
        };

        let json = serde_json::to_string(&zone).expect("zone serializes");
        let restored: Zone = serde_json::from_str(&json).expect("zone deserializes");

        assert_eq!(restored, zone, "a zone survives a round trip");
    }
}
