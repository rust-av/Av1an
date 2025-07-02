use super::*;
use crate::{encoder::Encoder, into_vec, scenes::ZoneOptions};

#[test]
fn test_extra_split_no_segments() {
    let split_size = 240;
    let done = extra_splits(
        &[Scene {
            start_frame:    0,
            end_frame:      300,
            zone_overrides: None,
        }],
        split_size,
        &BTreeMap::new(),
    );
    let expected_split_locations = vec![0usize, 150];

    assert_eq!(
        expected_split_locations,
        done.into_iter().map(|done| done.start_frame).collect::<Vec<usize>>()
    );
}

#[test]
fn test_extra_split_segments() {
    let split_size = 130;
    let done = extra_splits(
        &[
            Scene {
                start_frame:    0,
                end_frame:      150,
                zone_overrides: None,
            },
            Scene {
                start_frame:    150,
                end_frame:      460,
                zone_overrides: None,
            },
            Scene {
                start_frame:    460,
                end_frame:      728,
                zone_overrides: None,
            },
            Scene {
                start_frame:    728,
                end_frame:      822,
                zone_overrides: None,
            },
            Scene {
                start_frame:    822,
                end_frame:      876,
                zone_overrides: None,
            },
            Scene {
                start_frame:    876,
                end_frame:      890,
                zone_overrides: None,
            },
            Scene {
                start_frame:    890,
                end_frame:      1100,
                zone_overrides: None,
            },
            Scene {
                start_frame:    1100,
                end_frame:      1399,
                zone_overrides: None,
            },
            Scene {
                start_frame:    1399,
                end_frame:      1709,
                zone_overrides: None,
            },
            Scene {
                start_frame:    1709,
                end_frame:      2000,
                zone_overrides: None,
            },
        ],
        split_size,
        &BTreeMap::new(),
    );
    let expected_split_locations = [
        0usize, 75, 150, 253, 356, 460, 549, 638, 728, 822, 876, 890, 995, 1100, 1199, 1299, 1399,
        1502, 1605, 1709, 1806, 1903,
    ];

    assert_eq!(
        expected_split_locations,
        done.into_iter().map(|done| done.start_frame).collect::<Vec<usize>>().as_slice()
    );
}

#[test]
fn test_extra_split_preserves_zone_overrides() {
    let split_size = 130;
    let done = extra_splits(
        &[
            Scene {
                start_frame:    0,
                end_frame:      150,
                zone_overrides: None,
            },
            Scene {
                start_frame:    150,
                end_frame:      460,
                zone_overrides: None,
            },
            Scene {
                start_frame:    460,
                end_frame:      728,
                zone_overrides: Some(ZoneOptions {
                    encoder:             Encoder::rav1e,
                    passes:              1,
                    extra_splits_len:    Some(50),
                    min_scene_len:       12,
                    photon_noise:        None,
                    photon_noise_height: None,
                    photon_noise_width:  None,
                    chroma_noise:        false,
                    video_params:        into_vec!["--speed", "8"],
                }),
            },
            Scene {
                start_frame:    728,
                end_frame:      822,
                zone_overrides: None,
            },
            Scene {
                start_frame:    822,
                end_frame:      876,
                zone_overrides: None,
            },
            Scene {
                start_frame:    876,
                end_frame:      890,
                zone_overrides: None,
            },
            Scene {
                start_frame:    890,
                end_frame:      1100,
                zone_overrides: None,
            },
            Scene {
                start_frame:    1100,
                end_frame:      1399,
                zone_overrides: None,
            },
            Scene {
                start_frame:    1399,
                end_frame:      1709,
                zone_overrides: Some(ZoneOptions {
                    encoder:             Encoder::rav1e,
                    passes:              1,
                    extra_splits_len:    Some(split_size),
                    min_scene_len:       12,
                    photon_noise:        None,
                    photon_noise_height: None,
                    photon_noise_width:  None,
                    chroma_noise:        false,
                    video_params:        into_vec!["--speed", "3"],
                }),
            },
            Scene {
                start_frame:    1709,
                end_frame:      2000,
                zone_overrides: None,
            },
        ],
        split_size,
        &BTreeMap::new(),
    );
    let expected_split_locations = [
        0, 75, 150, 253, 356, 460, 504, 549, 594, 638, 683, 728, 822, 876, 890, 995, 1100, 1199,
        1299, 1399, 1502, 1605, 1709, 1806, 1903,
    ];

    for (i, scene) in done.into_iter().enumerate() {
        assert_eq!(scene.start_frame, expected_split_locations[i]);
        match scene.start_frame {
            460..=727 => {
                assert!(scene.zone_overrides.is_some());
                let overrides = scene.zone_overrides.unwrap();
                assert_eq!(overrides.video_params, vec![
                    "--speed".to_owned(),
                    "8".to_owned()
                ]);
            },
            1399..=1708 => {
                assert!(scene.zone_overrides.is_some());
                let overrides = scene.zone_overrides.unwrap();
                assert_eq!(overrides.video_params, vec![
                    "--speed".to_owned(),
                    "3".to_owned()
                ]);
            },
            _ => {
                assert!(scene.zone_overrides.is_none());
            },
        }
    }
}
