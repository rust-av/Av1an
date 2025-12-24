use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotonNoise {
    pub iso:        u32,
    pub chroma_iso: Option<u32>,
    pub width:      Option<u32>,
    pub height:     Option<u32>,
    pub c_y:        Option<Vec<i8>>,
    pub ccb:        Option<Vec<i8>>,
    pub ccr:        Option<Vec<i8>>,
}

impl Hash for PhotonNoise {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        serde_json::to_vec(self).expect("PhotonNoise should serialize").hash(state);
    }
}

impl PhotonNoise {
    #[inline]
    pub fn hash_name(&self) -> String {
        PhotonNoise::hash_full(self)
    }

    #[inline]
    pub fn hash_full(photon_noise: &PhotonNoise) -> String {
        let mut hasher = DefaultHasher::new();
        photon_noise.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}
