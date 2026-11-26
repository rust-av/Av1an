//! Public pure pooling contracts retained from the former correctness target.
use super::*;

#[test]
fn ssimulacra2_pooling_returns_requested_mean_min_or_max() {
    let scores = [
        FrameScore {
            score: Some(0.8),
            ..FrameScore::default()
        },
        FrameScore {
            score: Some(0.4),
            ..FrameScore::default()
        },
        FrameScore {
            score: Some(0.6),
            ..FrameScore::default()
        },
    ];
    let mean =
        VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean, true).unwrap();
    assert!((mean - 0.6).abs() < 1e-12, "mean was {mean}, expected 0.6");
    assert_eq!(
        VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Max, true).unwrap(),
        0.8
    );
    assert_eq!(
        VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Min, true).unwrap(),
        0.4
    );
}

#[test]
fn ssimulacra2_pooling_rejects_empty_scores() {
    assert!(VshipScorer::pool(&[], VshipMetric::Ssimulacra2, PoolMethod::Mean, true).is_err());
}
