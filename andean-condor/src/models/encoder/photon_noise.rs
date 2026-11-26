use std::hash::{DefaultHasher, Hash, Hasher};

use av1_grain::GrainTableSegment;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::EncoderSettingsError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhotonNoise {
    pub iso: u32,
    pub chroma_iso: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub c_y: Option<Vec<i8>>,
    pub ccb: Option<Vec<i8>>,
    pub ccr: Option<Vec<i8>>,
}

impl Hash for PhotonNoise {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        serde_json::to_vec(self).expect("PhotonNoise should serialize").hash(state);
    }
}

impl PhotonNoise {
    /// Check supported ISO settings and effective shapes using av1-grain 0.5's lag-0 defaults,
    /// without generating scaling points or touching an executable/output.
    pub(crate) fn validate_settings(&self) -> Result<(), EncoderSettingsError> {
        if let Some(chroma_iso) = self.chroma_iso
            && chroma_iso != self.iso
        {
            return Err(EncoderSettingsError::UnequalChromaIso {
                iso: self.iso,
                chroma_iso,
            });
        }
        self.coefficient_lag(0, 1, 1).map(|_| ())
    }

    fn coefficient_lag(&self, y: usize, cb: usize, cr: usize) -> Result<u8, EncoderSettingsError> {
        let y = self.c_y.as_ref().map_or(y, Vec::len);
        let lag = match y {
            0 => 0,
            4 => 1,
            12 => 2,
            24 => 3,
            _ => return Err(EncoderSettingsError::NoiseLumaCoefficients(y)),
        };
        for (name, actual) in [
            ("ccb", self.ccb.as_ref().map_or(cb, Vec::len)),
            ("ccr", self.ccr.as_ref().map_or(cr, Vec::len)),
        ] {
            if actual != y + 1 {
                return Err(EncoderSettingsError::NoiseChromaCoefficients {
                    name,
                    lag,
                    expected: y + 1,
                    actual,
                });
            }
        }
        Ok(lag)
    }

    /// Apply only after validating every effective vector; never pad user data.
    /// Both the text-table and native FGS routes use this contract.
    pub(crate) fn apply_coefficients(
        &self,
        params: &mut GrainTableSegment,
    ) -> Result<(), EncoderSettingsError> {
        // av1-grain 0.5 always generates luma scaling points, even at ISO 0.
        // AV1's N=2*lag*(lag+1); chroma has N+1 coefficients with luma points,
        // otherwise N. It signals chroma AR only for active planes (including
        // chroma-from-luma), and omits it for monochrome.
        // The installed TEXT grammar instead always reads Y=N, Cb/Cr=N+1,
        // including inactive slots. Keep both routes coherent with that format;
        // this helper does not support arbitrary no-luma-point grain segments.
        if params.scaling_points_y.is_empty() {
            return Err(EncoderSettingsError::NoiseWithoutLumaPoints);
        }
        let lag = self.coefficient_lag(
            params.ar_coeffs_y.len(),
            params.ar_coeffs_cb.len(),
            params.ar_coeffs_cr.len(),
        )?;
        // The exact arities above also prove these bounded collections fit.
        if let Some(values) = &self.c_y {
            params.ar_coeffs_y = values.iter().copied().collect();
        }
        if let Some(values) = &self.ccb {
            params.ar_coeffs_cb = values.iter().copied().collect();
        }
        if let Some(values) = &self.ccr {
            params.ar_coeffs_cr = values.iter().copied().collect();
        }
        params.ar_coeff_lag = lag;
        Ok(())
    }

    #[inline]
    pub fn hash_name(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::PhotonNoise;

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

    /// Value equality must match the manual `Hash` (which serializes), so a
    /// re-encoded config compares equal to the one it came from.
    #[test]
    fn photon_noise_compares_by_value() {
        assert_eq!(photon_noise(600), photon_noise(600));
        assert_ne!(photon_noise(600), photon_noise(12000));
    }

    #[test]
    fn photon_noise_equality_includes_chroma_iso() {
        let mut scaled = photon_noise(600);
        scaled.chroma_iso = Some(4000);
        assert_ne!(photon_noise(600), scaled);
    }
}
