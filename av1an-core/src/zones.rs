use std::fs;

use anyhow::bail;

use crate::{scenes::Scene, EncodeArgs};

pub(crate) fn parse_zones(args: &EncodeArgs, frames: usize) -> anyhow::Result<Vec<Scene>> {
    let mut zones = Vec::new();
    if let Some(ref zones_file) = args.zones {
        let input = fs::read_to_string(zones_file)?;
        let mut errors: Vec<String> = Vec::new();

        for (line_number, zone_line) in input
            .lines()
            .enumerate()
            .map(|(n, l)| (n + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty())
        {
            match Scene::parse_from_zone(zone_line, args, frames) {
                Ok(zone) => zones.push(zone),
                Err(e) => {
                    let error_msg = format!(
                        "Line {} \"{}\":\n  {}",
                        line_number,
                        zone_line,
                        e.to_string().replace('\n', "\n  ")
                    );
                    errors.push(error_msg);
                },
            }
        }

        if !errors.is_empty() {
            bail!(
                "Zone file validation failed with {} error(s):\n\n{}",
                errors.len(),
                errors.join("\n\n")
            );
        }
        zones.sort_unstable_by_key(|zone| zone.start_frame);
        for i in 0..zones.len() - 1 {
            let current_zone = &zones[i];
            let next_zone = &zones[i + 1];
            if current_zone.end_frame > next_zone.start_frame {
                bail!("Zones file contains overlapping zones");
            }
        }
    }
    Ok(zones)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encoder::Encoder, settings::EncodeArgs, ChunkMethod, Input};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_args_with_zones_file(content: &str) -> (EncodeArgs, TempDir) {
        use crate::{
            concat::ConcatMethod,
            ffmpeg::FFPixelFormat,
            into_vec,
            settings::{InputPixelFormat, PixelFormat},
            ChunkOrdering, ScenecutMethod, SplitMethod, Verbosity,
        };

        let temp_dir = TempDir::new().unwrap();
        let zones_file = temp_dir.path().join("test_zones.txt");
        fs::write(&zones_file, content).unwrap();

        let args = EncodeArgs {
            ffmpeg_filter_args: Vec::new(),
            temp: String::new(),
            force: false,
            no_defaults: false,
            passes: 2,
            video_params: into_vec!["--cq-level=40", "--cpu-used=0", "--aq-mode=1"],
            output_file: String::new(),
            audio_params: Vec::new(),
            chunk_method: ChunkMethod::LSMASH,
            chunk_order: ChunkOrdering::Random,
            concat: ConcatMethod::FFmpeg,
            encoder: Encoder::aom,
            extra_splits_len: Some(100),
            photon_noise: Some(10),
            photon_noise_size: (None, None),
            chroma_noise: false,
            sc_pix_format: None,
            keep: false,
            max_tries: 3,
            min_scene_len: 10,
            input_pix_format: InputPixelFormat::FFmpeg {
                format: FFPixelFormat::YUV420P10LE,
            },
            input: Input::Video {
                path: PathBuf::new(),
                temp: String::new(),
                chunk_method: ChunkMethod::LSMASH,
                is_proxy: false,
            },
            proxy: None,
            output_pix_format: PixelFormat {
                format: FFPixelFormat::YUV420P10LE,
                bit_depth: 10,
            },
            resume: false,
            scenes: None,
            split_method: SplitMethod::AvScenechange,
            sc_method: ScenecutMethod::Standard,
            sc_only: false,
            sc_downscale_height: None,
            force_keyframes: Vec::new(),
            target_quality: None,
            vmaf: false,
            verbosity: Verbosity::Normal,
            workers: 1,
            tiles: (1, 1),
            tile_auto: false,
            set_thread_affinity: None,
            zones: Some(zones_file),
            scaler: String::new(),
            ignore_frame_mismatch: false,
            vmaf_path: None,
            vmaf_res: "1920x1080".to_string(),
            vmaf_threads: None,
            vmaf_filter: None,
            probe_res: None,
            vapoursynth_plugins: None,
        };

        (args, temp_dir)
    }

    #[test]
    fn test_parse_zones_collects_errors_from_all_lines() {
        let zones_content = r#"0 10 aom --cq-level=30
20 15 aom --cq-level=25
30 150 aom --cq-level=35
40 50 x264 reset"#;

        let (args, _temp_dir) = create_test_args_with_zones_file(zones_content);
        let result = parse_zones(&args, 100);

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();

        // Should report all 3 errors with line numbers
        assert!(error_message.contains("Zone file validation failed with 3 error(s)"));
        assert!(error_message.contains("Line 2"));
        assert!(error_message.contains("Start frame must be earlier than the end frame"));
        assert!(error_message.contains("Line 3"));
        assert!(
            error_message.contains("Start and end frames must not be past the end of the video")
        );
        assert!(error_message.contains("Line 4"));
        assert!(error_message.contains("Zone specifies using x264"));
    }
}
