use super::california;
use andean_condor::models::{encoder::Encoder, scene::Scene};
use california::configuration::CliSequenceData;

/// Ordered cumulative prefixes for saved/live quality UI checks; no decoder or GPU.
pub fn quality_scene_fixture() -> (
    andean_condor::core::input::clip_info::ClipInfo,
    Vec<Scene<CliSequenceData>>,
) {
    use andean_condor::{
        core::input::{clip_info::ClipInfo, pixel_format::PixelFormat},
        models::sequence::target_quality::types::QualityPass,
    };
    let clip = ClipInfo {
        num_frames: 9,
        format_info: PixelFormat::VapourSynth { bit_depth: 8 },
        frame_rate: av_format::rational::Rational64::new(24, 1),
        resolution: (32, 32),
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    };
    let scenes = [vec![2.0, 9.0, 4.0], vec![1.0, 8.0, 6.0], vec![]]
        .into_iter()
        .enumerate()
        .map(|(i, scores)| {
            let pass = QualityPass {
                provenance: None,
                backend: None,
                quantizer: 20.0,
                scores,
                bitrate: 123.0,
                started_on: 100,
                completed_on: 200,
            };
            let mut sequence_data = CliSequenceData::default();
            sequence_data.quality_check.quality = if pass.scores.is_empty() {
                QualityPass::default()
            } else {
                pass.clone()
            };
            sequence_data.target_quality.passes = if pass.scores.is_empty() {
                vec![]
            } else {
                vec![
                    pass.clone(),
                    QualityPass {
                        quantizer: 30.0,
                        ..pass
                    },
                ]
            };
            Scene {
                start_frame: i * 3,
                end_frame: (i + 1) * 3,
                sub_scenes: None,
                encoder: Encoder::default(),
                sequence_data,
            }
        })
        .collect();
    (clip, scenes)
}

pub(crate) fn run_resumed_quality_report() -> california::apps::quality_check::QualityCheckApp {
    use andean_condor::{
        core::sequence::{SequenceCompletion, SequenceStatus, Status},
        models::sequence::target_quality::types::{ProbeStatistic, ProbeStrategy, QualityMetric},
    };
    use california::apps::{TuiApp, quality_check::QualityCheckApp};

    let (clip, mut scenes) = quality_scene_fixture();
    scenes[0].sequence_data.quality_check.quality.scores = (0..100).map(f64::from).collect();
    scenes[0].end_frame = 100;
    scenes[1].start_frame = 100;
    scenes[1].end_frame = 200;
    scenes.truncate(2);
    scenes[1].sequence_data.quality_check.quality = Default::default();
    let mut app = QualityCheckApp::new(
        clip,
        &scenes,
        QualityMetric::default(),
        ProbeStatistic::Mean,
        &ProbeStrategy::Whole,
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    for frame in 100..200 {
        tx.send(SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: "Quality Check".into(),
                completion: SequenceCompletion::Custom {
                    name: "Quality Check".into(),
                    completed: (frame - 100) as f64 + 1.0,
                    total: 100.0,
                },
            },
            child: Status::Processing {
                id: "Quality".into(),
                completion: SequenceCompletion::FrameScore {
                    frame,
                    score: frame as f64,
                },
            },
        })
        .unwrap();
    }
    tx.send(SequenceStatus::Subprocess {
        parent: Status::Processing {
            id: "Quality Check".into(),
            completion: SequenceCompletion::Custom {
                name: "Quality Check".into(),
                completed: 100.0,
                total: 100.0,
            },
        },
        child: Status::Processing {
            id: "Quality".into(),
            completion: SequenceCompletion::SceneQuality {
                index: 1,
                score: 149.5,
                quantizer: 20.0,
                bitrate: 0.0,
            },
        },
    })
    .unwrap();
    drop(tx);
    app.run(
        rx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    app
}
