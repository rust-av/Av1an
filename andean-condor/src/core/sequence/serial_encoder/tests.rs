use super::*;
#[cfg(target_os = "linux")]
use crate as condor;
#[cfg(target_os = "linux")]
#[path = "../../../../tests/core/sequence/serial_encoder/fixtures/artifact_support.rs"]
mod fixture;

#[test]
fn serial_relay_keeps_draining_after_progress_receiver_disconnects() {
    for count in [0, 1, 9, 18] {
        let (source, rx) = sync::mpsc::sync_channel(0);
        let (sink, disconnected) = sync::mpsc::channel();
        drop(disconnected);
        let relay = thread::spawn(move || {
            relay_progress(rx, sink);
            Ok(())
        });
        for frame in 0..count {
            source
                .send(EncodeProgress {
                    pass: (1, 2),
                    frame,
                    usage: Default::default(),
                })
                .expect("relay keeps draining after downstream disconnect");
        }
        drop(source);
        join_worker(relay, "serial relay regression").expect("relay joined at EOF");
    }
}

#[test]
fn serial_inner_errors_preserve_all_causes() {
    let error = serial_results([
        Err(anyhow::anyhow!("encode failure")),
        Err(anyhow::anyhow!("decode failure")),
        Err(anyhow::anyhow!("relay failure")),
    ])
    .expect_err("inner errors cannot be discarded");
    let text = format!("{error:#}");
    for cause in ["encode failure", "decode failure", "relay failure"] {
        assert!(text.contains(cause), "{text}");
    }
}

#[cfg(target_os = "linux")]
pub(super) mod internal_integration {
    use super::*;

    pub(in crate::core::sequence::serial_encoder) fn worker(name: &str) {
        assert!(
            std::env::var("ENCODER_FIXTURE_CASE").as_deref() != Ok(&format!("{name}-panic")),
            "serial-encoder deliberate {name} panic"
        );
    }

    #[test]
    fn serial_worker_panics_are_joined_before_failed_output_cleanup() {
        fixture::serial_cases(
            "core::sequence::serial_encoder::tests::internal_integration::serial_worker_panics_are_joined_before_failed_output_cleanup",
            &["feed-panic", "encoder-panic", "relay-panic"],
        );
    }
}
