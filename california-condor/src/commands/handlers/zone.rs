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
    use crate::{test_helpers::default_config, zoner::ZoneEncoder};

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
