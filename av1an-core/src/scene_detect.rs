use std::{
    io::{IsTerminal, Read},
    process::{Command, Stdio},
    thread,
};

use ansi_term::Style;
use anyhow::bail;
use av_decoders::{DecoderError, DecoderImpl, FfmpegDecoder, VapoursynthDecoder, Y4mDecoder};
use av_scenechange::{detect_scene_changes, Decoder, DetectionOptions, SceneDetectionSpeed};
use ffmpeg::format::Pixel;
use itertools::Itertools;
use smallvec::{smallvec, SmallVec};
use vapoursynth::video_info::Property;

use crate::{
    into_smallvec,
    progress_bar,
    scenes::Scene,
    util::to_absolute_path,
    vapoursynth::resize_node,
    Encoder,
    Input,
    ScenecutMethod,
    Verbosity,
};

#[tracing::instrument(level = "debug")]
#[allow(clippy::too_many_arguments)]
pub fn av_scenechange_detect(
    input: &Input,
    encoder: Encoder,
    total_frames: usize,
    min_scene_len: usize,
    verbosity: Verbosity,
    sc_scaler: &str,
    sc_pix_format: Option<Pixel>,
    sc_method: ScenecutMethod,
    sc_downscale_height: Option<usize>,
    zones: &[Scene],
) -> anyhow::Result<(Vec<Scene>, usize)> {
    if verbosity != Verbosity::Quiet {
        if std::io::stderr().is_terminal() {
            eprintln!("{}", Style::default().bold().paint("Scene detection"));
        } else {
            eprintln!("Scene detection");
        }
        progress_bar::init_progress_bar(total_frames as u64, 0, None);
    }

    let input2 = input.clone();
    let frame_thread = thread::spawn(move || {
        let frames = input2.frames(None).unwrap();
        if verbosity != Verbosity::Quiet {
            progress_bar::convert_to_progress(0);
            progress_bar::set_len(frames as u64);
        }
        frames
    });

    let frames = frame_thread.join().unwrap();
    let scenes = scene_detect(
        input,
        encoder,
        total_frames,
        if verbosity == Verbosity::Quiet {
            None
        } else {
            Some(&|frames| {
                progress_bar::set_pos(frames as u64);
            })
        },
        min_scene_len,
        sc_scaler,
        sc_pix_format,
        sc_method,
        sc_downscale_height,
        zones,
    )?;

    progress_bar::finish_progress_bar();

    Ok((scenes, frames))
}

/// Detect scene changes using rav1e scene detector.
#[allow(clippy::too_many_arguments)]
pub fn scene_detect(
    input: &Input,
    encoder: Encoder,
    total_frames: usize,
    callback: Option<&dyn Fn(usize)>,
    min_scene_len: usize,
    sc_scaler: &str,
    sc_pix_format: Option<Pixel>,
    sc_method: ScenecutMethod,
    sc_downscale_height: Option<usize>,
    zones: &[Scene],
) -> anyhow::Result<Vec<Scene>> {
    let (mut decoder, bit_depth) = build_decoder(
        input,
        encoder,
        sc_scaler,
        sc_pix_format,
        sc_downscale_height,
    )?;

    let mut scenes = Vec::new();
    let mut cur_zone = zones.first().filter(|frame| frame.start_frame == 0);
    let mut next_zone_idx = if zones.is_empty() {
        None
    } else if cur_zone.is_some() {
        if zones.len() == 1 {
            None
        } else {
            Some(1)
        }
    } else {
        Some(0)
    };
    let mut frames_read = 0;
    loop {
        let mut min_scene_len = min_scene_len;
        if let Some(zone) = cur_zone {
            if let Some(ref overrides) = zone.zone_overrides {
                min_scene_len = overrides.min_scene_len;
            }
        };
        let options = DetectionOptions {
            min_scenecut_distance: Some(min_scene_len),
            analysis_speed: match sc_method {
                ScenecutMethod::Fast => SceneDetectionSpeed::Fast,
                ScenecutMethod::Standard => SceneDetectionSpeed::Standard,
            },
            ..DetectionOptions::default()
        };
        let frame_limit = if let Some(zone) = cur_zone {
            Some(zone.end_frame - zone.start_frame)
        } else if let Some(next_idx) = next_zone_idx {
            let zone = &zones[next_idx];
            Some(zone.start_frame - frames_read)
        } else {
            None
        };
        let callback = callback.map(|cb| {
            |frames, _keyframes| {
                cb(frames + frames_read);
            }
        });
        let sc_result = if bit_depth > 8 {
            detect_scene_changes::<u16>(
                &mut decoder,
                options,
                frame_limit,
                callback.as_ref().map(|cb| cb as &dyn Fn(usize, usize)),
            )
        } else {
            detect_scene_changes::<u8>(
                &mut decoder,
                options,
                frame_limit,
                callback.as_ref().map(|cb| cb as &dyn Fn(usize, usize)),
            )
        }?;
        if let Some(limit) = frame_limit {
            if limit != sc_result.frame_count {
                bail!(
                    "Scene change: Expected {} frames but saw {}. This may indicate an issue with \
                     the input or filters.",
                    limit,
                    sc_result.frame_count
                );
            }
        }
        let scene_changes = sc_result.scene_changes;
        for (start, end) in scene_changes.iter().copied().tuple_windows() {
            scenes.push(Scene {
                start_frame:    start + frames_read,
                end_frame:      end + frames_read,
                zone_overrides: cur_zone.and_then(|zone| zone.zone_overrides.clone()),
            });
        }

        scenes.push(Scene {
            start_frame:    scenes.last().map(|scene| scene.end_frame).unwrap_or_default(),
            end_frame:      if let Some(limit) = frame_limit {
                frames_read += limit;
                frames_read
            } else {
                total_frames
            },
            zone_overrides: cur_zone.and_then(|zone| zone.zone_overrides.clone()),
        });
        if let Some(next_idx) = next_zone_idx {
            if cur_zone.is_none_or(|zone| zone.end_frame == zones[next_idx].start_frame) {
                cur_zone = Some(&zones[next_idx]);
                next_zone_idx = if next_idx + 1 == zones.len() {
                    None
                } else {
                    Some(next_idx + 1)
                };
            } else {
                cur_zone = None;
            }
        } else if cur_zone.is_none_or(|zone| zone.end_frame == total_frames) {
            // End of video
            break;
        } else {
            cur_zone = None;
        }
    }
    Ok(scenes)
}

#[tracing::instrument(level = "debug")]
fn build_decoder(
    input: &Input,
    encoder: Encoder,
    sc_scaler: &str,
    sc_pix_format: Option<Pixel>,
    sc_downscale_height: Option<usize>,
) -> anyhow::Result<(Decoder, usize)> {
    let bit_depth;
    let filters: SmallVec<[String; 4]> = match (sc_downscale_height, sc_pix_format) {
        (Some(sdh), Some(spf)) => into_smallvec![
            "-vf",
            format!(
                "format={},scale=-2:'min({},ih)':flags={}",
                spf.descriptor().unwrap().name(),
                sdh,
                sc_scaler
            )
        ],
        (Some(sdh), None) => {
            into_smallvec!["-vf", format!("scale=-2:'min({sdh},ih)':flags={sc_scaler}")]
        },
        (None, Some(spf)) => into_smallvec!["-pix_fmt", spf.descriptor().unwrap().name()],
        (None, None) => smallvec![],
    };

    let decoder = if input.is_vapoursynth() && filters.is_empty() {
        // No downscaling or format conversion for user-provided VapourSynth script
        // VSPipe is still faster than VapoursynthDecoder
        bit_depth = crate::vapoursynth::bit_depth(input.as_path(), input.as_vspipe_args_map()?)?;
        let mut command = Command::new("vspipe");
        // input path might be relative, get the absolute path
        let path = to_absolute_path(input.as_vapoursynth_path())?;
        command
            // Set the CWD to the directory of the user-provided VapourSynth script
            .current_dir(path.parent().unwrap())
            .arg("-c")
            .arg("y4m")
            .arg(path)
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Append vspipe python arguments to the environment if there are any
        for arg in input.as_vspipe_args_vec()? {
            command.args(["-a", &arg]);
        }

        let y4m_decoder =
            Y4mDecoder::new(Box::new(command.spawn()?.stdout.unwrap()) as Box<dyn Read>)?;
        Decoder::from_decoder_impl(DecoderImpl::Y4m(y4m_decoder))?
    } else if input.is_vapoursynth_script() {
        // Downscaling may be necessary and VapoursynthDecoder is the most reliable

        let mut vs_decoder = if input.is_vapoursynth() {
            // Must use from_file in order to set the CWD to the
            // directory of the user-provided VapourSynth script
            VapoursynthDecoder::from_file(input.as_vapoursynth_path())?
        } else {
            // Loadscript text generated in memory, CWD is inherited
            VapoursynthDecoder::from_script(&input.as_script_text()?)?
        };
        vs_decoder.set_variables(input.as_vspipe_args_hashmap()?)?;

        if let Some(height) = sc_downscale_height {
            // Register a node modifier callback to perform downscaling
            vs_decoder.register_node_modifier(Box::new(move |core, node| {
                // Node is expected to exist
                let node = node.ok_or_else(|| DecoderError::VapoursynthInternalError {
                    cause: "No output node".to_string(),
                })?;
                let info = node.info();
                let resolution = {
                    match info.resolution {
                        Property::Variable => {
                            return Err(DecoderError::VariableResolution);
                        },
                        Property::Constant(x) => x,
                    }
                };
                let original_width = resolution.width;
                let original_height = resolution.height;

                if original_height <= height {
                    // Specified downscale height is less than or equal
                    // to original height, return the original node unmodified
                    return Ok(node);
                }

                let width = (original_width as f64 * (height as f64 / original_height as f64))
                    .round() as u32;

                let resized_node = resize_node(
                    core,
                    &node,
                    Some((width / 2) * 2), // Ensure width is divisible by 2
                    Some(height as u32),
                    None,
                    None,
                )
                .map_err(|e| DecoderError::VapoursynthInternalError {
                    cause: e.to_string(),
                })?;

                Ok(resized_node)
            }))?;
        }

        let decoder = Decoder::from_decoder_impl(DecoderImpl::Vapoursynth(vs_decoder))?;
        bit_depth = decoder.get_video_details().bit_depth;

        decoder
    } else {
        // FFmpeg piped into Y4mDecoder
        let path = input.as_path();
        let input_pix_format = crate::ffmpeg::get_pixel_format(path)
            .unwrap_or_else(|e| panic!("FFmpeg failed to get pixel format for input video: {e:?}"));

        bit_depth = encoder.get_format_bit_depth(sc_pix_format.unwrap_or(input_pix_format))?;
        let decoder_impl = if filters.is_empty() {
            DecoderImpl::Ffmpeg(FfmpegDecoder::new(path)?)
        } else {
            let stdout = Command::new("ffmpeg")
                .args(["-r", "1", "-i"])
                .arg(path)
                .args(filters.as_ref())
                .args(["-f", "yuv4mpegpipe", "-strict", "-1", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?
                .stdout
                .expect("FFmpeg failed to output");

            DecoderImpl::Y4m(Y4mDecoder::new(Box::new(stdout) as Box<dyn Read>)?)
        };

        Decoder::from_decoder_impl(decoder_impl)?
    };

    Ok((decoder, bit_depth))
}
