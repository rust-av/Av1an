use crate::core::encoder::{EncodeResourceUsage, ResourceUsageTracker};

const MIB: u64 = 1024 * 1024;

fn usage(cpu: f32, memory_mib: u64) -> EncodeResourceUsage {
    EncodeResourceUsage {
        cpu,
        memory: memory_mib * MIB,
    }
}

#[test]
fn a_default_sample_is_unavailable_while_either_value_marks_it_available() {
    assert!(!EncodeResourceUsage::default().is_available());
    assert!(
        usage(412.0, 0).is_available(),
        "cpu alone is still a real sample"
    );
    assert!(
        usage(0.0, 512).is_available(),
        "memory alone is still a real sample"
    );
    assert_eq!(usage(412.0, 1536).mib(), 1536);
    assert_eq!(
        EncodeResourceUsage {
            cpu: 0.0,
            memory: 2 * MIB - 1,
        }
        .mib(),
        1,
        "mebibytes round down, never up past the measured bytes"
    );
}

#[test]
fn peak_sums_the_scenes_active_at_the_same_time() {
    let tracker = ResourceUsageTracker::default();
    tracker.report(0, usage(100.0, 100));
    tracker.report(1, usage(100.0, 50));
    assert_eq!(tracker.peak_memory(), 150 * MIB);
    assert_eq!(tracker.peak_cpu(), 200.0);

    tracker.report(0, usage(10.0, 40));
    assert_eq!(
        tracker.peak_memory(),
        150 * MIB,
        "a smaller later sum does not lower the high-water mark"
    );
    assert_eq!(tracker.peak_cpu(), 200.0);
}

#[test]
fn retiring_a_scene_stops_counting_its_last_sample() {
    let tracker = ResourceUsageTracker::default();
    tracker.report(0, usage(0.0, 100));
    tracker.report(1, usage(0.0, 100));
    tracker.retire(0);
    tracker.report(2, usage(0.0, 150));

    assert_eq!(
        tracker.peak_memory(),
        250 * MIB,
        "the retired scene's stale sample is gone"
    );
}

#[test]
fn repeated_samples_for_one_scene_do_not_double_count() {
    let tracker = ResourceUsageTracker::default();
    tracker.report(7, usage(0.0, 100));
    tracker.report(7, usage(0.0, 100));
    tracker.report(7, usage(0.0, 100));

    assert_eq!(tracker.peak_memory(), 100 * MIB);
}
