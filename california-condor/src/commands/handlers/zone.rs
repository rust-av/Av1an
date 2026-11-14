use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{
    commands::handlers::load_configuration,
    configuration::Configuration,
    zoner::{ZonerConfig, parse_zones},
};

/// Loads and configures zones in memory. Zoner saves only after successful
/// validation and application.
pub fn zone_handler(
    config_path: Option<&Path>,
    zones: &[String],
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if zones.is_empty() {
        if configuration
            .condor
            .sequence_config
            .zoner
            .as_ref()
            .is_none_or(|zoner| zoner.zones.is_empty())
            && !configuration
                .condor
                .scenes
                .iter()
                .any(|s| s.sequence_data.zoner.zoned || s.sequence_data.zoner.lifecycle.is_some())
        {
            bail!(
                "no zones provided; pass a zone definition or a zone file, or add \
                 condor.sequence_config.zoner to the configuration"
            );
        }
    } else {
        configure_zoner(&mut configuration, zones)?;
    }

    Ok((configuration, config_path))
}

/// Replaces the configured zones with `zones`, each an inline zone or a path to
/// a zone file.
pub fn configure_zoner(configuration: &mut Configuration, zones: &[String]) -> Result<()> {
    let zones = parse_zones(zones)?;
    if zones.is_empty() {
        bail!("no zones provided");
    }
    configuration.condor.sequence_config.zoner = Some(ZonerConfig { zones });

    Ok(())
}

#[cfg(test)]
mod tests {
    use andean_condor::models::encoder::EncoderBase;

    use super::*;
    use crate::{
        test_helpers::{default_config, get_test_video},
        utils::hash_path::hash_path,
        zoner::ZoneEncoder,
    };

    #[test]
    fn a_zone_command_without_zones_keeps_the_configured_ones() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let mut configuration =
            default_config(&test_video, &temp.path().join("out.mkv"), &temp_abs);
        configuration.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![crate::zoner::parse_zone("-1 200 svt-av1").expect("the zone parses")],
        });
        configuration.save(&config_path).expect("the config saves");

        let (config, found_config_path) =
            zone_handler(Some(&config_path), &[]).expect("the handler succeeds");

        assert_eq!(
            found_config_path, config_path_abs,
            "the handler reports the config it loaded"
        );
        assert_eq!(
            config.condor.sequence_config.zoner, configuration.condor.sequence_config.zoner,
            "the configured zones survive"
        );
    }

    #[test]
    fn a_zone_command_without_zones_or_config_errors() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");
        default_config(&test_video, &temp.path().join("out.mkv"), &temp_abs)
            .save(&config_path)
            .expect("the config saves");

        let error = zone_handler(Some(&config_path), &[]).expect_err("there is nothing to zone");

        assert!(
            error.to_string().contains("no zones provided"),
            "error explains the missing zones: {error}"
        );
    }

    #[test]
    fn configure_zoner_preserves_requested_executable() {
        let root = std::env::temp_dir().join("condor-zone-executable-model-only");
        let test_video = crate::test_helpers::TestVideo {
            path: root.join("not-opened-input.mkv"),
            width: 2,
            height: 2,
            frames: 240,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut configuration = default_config(&test_video, &root.join("out.mkv"), &root);
        let binary = PathBuf::from("missing-zone-encoder");

        configure_zoner(
            &mut configuration,
            &[format!(
                "0 200 replace svt-av1(\"{}\") \"--preset 3\" --iso 400",
                binary.display()
            )],
        )
        .expect("the zone parses");

        let zoner = configuration.condor.sequence_config.zoner.expect("the zoner is configured");
        assert_eq!(zoner.zones.len(), 1);
        let Some(ZoneEncoder::Replace(encoder)) = &zoner.zones[0].encoder else {
            panic!("the zone replaces the encoder");
        };
        assert_eq!(encoder.base(), EncoderBase::SVTAV1);
        assert_eq!(encoder.executable(), binary);
        assert_eq!(
            encoder.photon_noise().map(|table| table.iso),
            Some(400),
            "the zone's Photon Noise reaches the encoder"
        );
    }
}
