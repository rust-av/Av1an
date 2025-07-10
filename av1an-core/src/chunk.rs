#[cfg(test)]
mod tests;

use std::{ffi::OsString, path::Path};

use av1_grain::{generate_photon_noise_params, write_grain_table, NoiseGenArgs};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{encoder::Encoder, settings::insert_noise_table_params, Input};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub temp:                  String,
    pub index:                 usize,
    pub input:                 Input,
    pub proxy:                 Option<Input>,
    pub source_cmd:            Vec<OsString>,
    pub proxy_cmd:             Option<Vec<OsString>>,
    pub output_ext:            String,
    pub start_frame:           usize,
    // End frame is exclusive, i.e. the range of frames is `start_frame..end_frame`
    pub end_frame:             usize,
    pub frame_rate:            f64,
    pub passes:                u8,
    pub video_params:          Vec<String>,
    pub encoder:               Encoder,
    pub noise_size:            (Option<u32>, Option<u32>),
    // do not break compatibility with output produced by older versions of av1an
    /// Optional target quality CQ level
    #[serde(rename = "per_shot_target_quality_cq")]
    pub tq_cq:                 Option<u32>,
    pub ignore_frame_mismatch: bool,
}

impl Chunk {
    /// Returns numeric name of chunk `00001`
    pub fn name(&self) -> String {
        format!("{:05}", self.index)
    }

    pub fn output(&self) -> String {
        Path::new(&self.temp)
            .join("encode")
            .join(format!("{}.{}", self.name(), self.output_ext))
            .to_str()
            .unwrap()
            .to_owned()
    }

    pub const fn frames(&self) -> usize {
        self.end_frame - self.start_frame
    }

    pub(crate) fn apply_photon_noise_args(
        &mut self,
        photon_noise: Option<u8>,
        chroma_noise: bool,
    ) -> anyhow::Result<()> {
        if let Some(strength) = photon_noise {
            let iso_setting = u32::from(strength) * 100;
            let grain_table = Path::new(&self.temp).join(format!("iso{iso_setting}-grain.tbl"));
            if !grain_table.exists() {
                debug!("Generating grain table at ISO {iso_setting}");
                let clip_info = self.input.clip_info()?;
                let (mut width, mut height) = clip_info.resolution;
                if self.noise_size.0.is_some() {
                    width = self.noise_size.0.unwrap();
                }
                if self.noise_size.1.is_some() {
                    height = self.noise_size.1.unwrap();
                }
                let transfer_function =
                    clip_info.transfer_function_params_adjusted(&self.video_params);
                let params = generate_photon_noise_params(0, u64::MAX, NoiseGenArgs {
                    iso_setting,
                    width,
                    height,
                    transfer_function,
                    chroma_grain: chroma_noise,
                    random_seed: None,
                });
                write_grain_table(&grain_table, &[params])?;
            }

            insert_noise_table_params(self.encoder, &mut self.video_params, &grain_table)?;
        }

        Ok(())
    }
}
