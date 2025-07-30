use std::fs;

use anyhow::bail;

use crate::{metrics::vmaf::validate_libvmaf, scenes::Scene, EncodeArgs, TargetMetric};

pub(crate) fn parse_zones(args: &EncodeArgs, frames: usize) -> anyhow::Result<Vec<Scene>> {
    let mut zones = Vec::new();
    if let Some(ref zones_file) = args.zones {
        let input = fs::read_to_string(zones_file)?;
        for zone_line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
            zones.push(Scene::parse_from_zone(zone_line, args, frames)?);
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

pub(crate) fn validate_zones(args: &EncodeArgs, zones: &[Scene]) -> anyhow::Result<()> {
    if zones.is_empty() {
        // No zones to validate
        return Ok(());
    }

    // Using VMAF, validate libvmaf
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| tq.target.as_ref().filter(|_| tq.metric == TargetMetric::VMAF))
            .is_some()
    }) {
        validate_libvmaf()?;
    }

    // Using SSIMULACRA2, validate SSIMULACRA2
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| tq.target.as_ref().filter(|_| tq.metric == TargetMetric::SSIMULACRA2))
            .is_some()
    }) {
        args.validate_ssimululacra2()?;
    }

    // Using butteraugli-Inf, validate butteraugli-Inf
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| tq.target.as_ref().filter(|_| tq.metric == TargetMetric::ButteraugliINF))
            .is_some()
    }) {
        args.validate_butteraugli_inf()?;
    }

    // Using butteraugli-3, validate butteraugli-3
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| tq.target.as_ref().filter(|_| tq.metric == TargetMetric::Butteraugli3))
            .is_some()
    }) {
        args.validate_butteraugli_3()?;
    }

    // Using XPSNR and a probing rate > 1, validate XPSNR
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| {
                tq.target.as_ref().filter(|_| {
                    matches!(tq.metric, TargetMetric::XPSNR | TargetMetric::XPSNRWeighted)
                        && tq.probing_rate > 1
                })
            })
            .is_some()
    }) {
        // Any value greater than 1, uses VapourSynth
        args.validate_xpsnr(TargetMetric::XPSNR, 2)?;
    }

    // Using XPSNR and a probing rate of 1, validate XPSNR
    if zones.iter().any(|zone| {
        zone.zone_overrides
            .as_ref()
            .and_then(|ovr| ovr.target_quality.as_ref())
            .and_then(|tq| {
                tq.target.as_ref().filter(|_| {
                    matches!(tq.metric, TargetMetric::XPSNR | TargetMetric::XPSNRWeighted)
                        && tq.probing_rate == 1
                })
            })
            .is_some()
    }) {
        // 1, uses FFmpeg
        args.validate_xpsnr(TargetMetric::XPSNR, 1)?;
    }

    Ok(())
}
