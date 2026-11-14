use andean_condor::{
    metrics::{Engine, engine, vship},
    models::sequence::target_quality::types::QualityMetric,
};

fn ssimulacra2() -> QualityMetric {
    QualityMetric::SSIMULACRA2 {
        target_range: (74.0, 76.0),
        resolution: None,
        threads: None,
        gpu_id: None,
    }
}

fn butteraugli() -> QualityMetric {
    QualityMetric::BUTTERAUGLI {
        target_range: (1.0, 3.0),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: None,
        gpu_id: None,
    }
}

fn xpsnr() -> QualityMetric {
    QualityMetric::XPSNR {
        target_range: (45.0, 55.0),
        resolution: None,
    }
}

#[test]
fn xpsnr_engine_selection_falls_back_on_the_current_machine() {
    assert_eq!(engine(&xpsnr(), true), None);
    assert_eq!(engine(&xpsnr(), false), None);
}

#[test]
fn available_vship_is_selected_for_both_input_kinds() {
    if vship::native_supported(&ssimulacra2()) {
        assert_eq!(engine(&ssimulacra2(), true), Some(Engine::Vship));
        assert_eq!(engine(&ssimulacra2(), false), Some(Engine::Vship));
    }
}

#[test]
fn vapoursynth_inputs_do_not_select_fmetrics_when_plugins_are_available() {
    assert_ne!(engine(&ssimulacra2(), true), Some(Engine::Fmetrics));
    assert_ne!(engine(&butteraugli(), true), Some(Engine::Fmetrics));
}
