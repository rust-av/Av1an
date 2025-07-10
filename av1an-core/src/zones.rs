use std::fs;

use anyhow::bail;

use crate::{scenes::Scene, EncodeArgs};

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
