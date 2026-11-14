use av_metrics_fmetrics::{FmetricsConfig, FmetricsMetric};
use av_metrics_vship::VshipConfig;

#[test]
fn cvvdp_temporal_disable_and_discontinuity_reset_flags_are_independent() {
    for disabled in [false, true] {
        for gap in [false, true] {
            let f = FmetricsConfig::new(FmetricsMetric::Cvvdp)
                .with_reset_on_discontinuity(gap)
                .with_disable_temporal(disabled);
            let v = VshipConfig::new()
                .with_reset_on_discontinuity(gap)
                .with_disable_temporal(disabled);
            assert_eq!(f.reset_on_discontinuity, gap);
            assert_eq!(v.cvvdp().reset_on_discontinuity, gap);
            assert_eq!(f.disable_temporal, disabled);
            assert_eq!(v.cvvdp().disable_temporal, disabled);
            assert_eq!(
                f.with_reset_on_discontinuity(!gap).disable_temporal,
                disabled
            );
            assert_eq!(
                v.with_reset_on_discontinuity(!gap).cvvdp().disable_temporal,
                disabled
            );
        }
    }
    assert!(!FmetricsConfig::new(FmetricsMetric::Cvvdp).disable_temporal);
    assert!(!VshipConfig::new().cvvdp().disable_temporal);
}
