use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, IntoStaticStr,
)]
pub enum ButteraugliSubMetric {
    #[strum(serialize = "butteraugli-infinite-norm")]
    InfiniteNorm,
    #[strum(serialize = "butteraugli-3-norm")]
    ThreeNorm,
}
