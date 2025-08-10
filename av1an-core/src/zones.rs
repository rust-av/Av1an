use std::fs;

use anyhow::bail;

use crate::{
    metrics::vmaf::validate_libvmaf,
    scenes::Scene,
    EncodeArgs,
    TargetMetric,
    TargetQuality,
};

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

    let tq_used_and = |condition: &dyn Fn(&TargetQuality) -> bool| {
        zones.iter().any(|zone| {
            zone.zone_overrides
                .as_ref()
                .and_then(|ovr| ovr.target_quality.as_ref())
                .and_then(|tq| tq.target.as_ref().filter(|_| condition(tq)))
                .is_some()
        })
    };
    let tq_used_and_metric_is = |metric: TargetMetric| tq_used_and(&|tq| tq.metric == metric);

    if tq_used_and_metric_is(TargetMetric::VMAF) {
        validate_libvmaf()?;
    }

    if tq_used_and_metric_is(TargetMetric::SSIMULACRA2) {
        args.validate_ssimulacra2()?;
    }

    // Using butteraugli-INF, validate butteraugli-INF
    if tq_used_and_metric_is(TargetMetric::ButteraugliINF) {
        args.validate_butteraugli_inf()?;
    }

    // Using butteraugli-3, validate butteraugli-3
    if tq_used_and_metric_is(TargetMetric::Butteraugli3) {
        args.validate_butteraugli_3()?;
    }

    // Using XPSNR and a probing rate > 1, validate XPSNR
    if tq_used_and(&|tq| {
        matches!(tq.metric, TargetMetric::XPSNR | TargetMetric::XPSNRWeighted)
            && tq.probing_rate > 1
    }) {
        // Any value greater than 1, uses VapourSynth
        args.validate_xpsnr(TargetMetric::XPSNR, 2)?;
    }

    // Using XPSNR and a probing rate of 1, validate XPSNR
    if tq_used_and(&|tq| {
        matches!(tq.metric, TargetMetric::XPSNR | TargetMetric::XPSNRWeighted)
            && tq.probing_rate == 1
    }) {
        // 1, uses FFmpeg
        args.validate_xpsnr(TargetMetric::XPSNR, 1)?;
    }

    Ok(())
}
