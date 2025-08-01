use std::fs;

use anyhow::bail;
use tracing::warn;

use crate::{scenes::Scene, EncodeArgs};

pub(crate) fn parse_zones(args: &EncodeArgs, frames: usize) -> anyhow::Result<Vec<Scene>> {
    let mut zones = Vec::new();
    if let Some(ref zones_file) = args.zones {
        let input = fs::read_to_string(zones_file)?;
        let mut warnings: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for (line_number, zone_line) in input
            .lines()
            .enumerate()
            .map(|(n, l)| (n + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty())
        {
            match Scene::parse_from_zone(zone_line, args, frames) {
                Ok((zone, warning_vec)) => {
                    if let Some(warnings_vec) = warning_vec {
                        let warning_msg = format!(
                            "Line {} \"{}\":\n  {}",
                            line_number,
                            zone_line,
                            warnings_vec.join("\n  ")
                        );
                        warnings.push(warning_msg);
                    }
                    zones.push(zone);
                },
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

        if !warnings.is_empty() {
            warn!(
                "Zone file validation had {} warning(s):\n\n{}",
                warnings.len(),
                warnings.join("\n\n")
            );
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
