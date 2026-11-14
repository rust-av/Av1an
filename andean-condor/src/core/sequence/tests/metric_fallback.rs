//! Capture the called production logging paths, not an isolated error formatter.
use std::{
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

use crate::{metrics::Engine, models::sequence::target_quality::types::QualityMetric};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<String>>);

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        write!(self.0, "{}={value:?};", field.name()).expect("write captured tracing field");
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut output = self.0.lock().expect("capture event");
        write!(
            output,
            "{}:{};",
            event.metadata().target(),
            event.metadata().level()
        )
        .expect("capture event metadata");
        event.record(&mut Fields(&mut output));
    }
}

fn assert_causes_emitted(target: &str, log: impl Fn(&anyhow::Error)) {
    for side in ["reference", "distorted"] {
        for (field, value) in [("transfer", "SMPTE2084"), ("primaries", "BT2020")] {
            let error = anyhow::Error::new(av_metrics_fmetrics::FmetricsError::UnsupportedFormat {
                reason: format!("unsupported {field} {value}"),
            })
            .context(format!("{side} color metadata"))
            .context("native scoring request");
            let capture = Capture::default();
            tracing::subscriber::with_default(capture.clone(), || log(&error));
            let output = capture.0.lock().expect("captured event");
            for expected in [
                target,
                "ERROR",
                "native scoring request",
                side,
                field,
                value,
                "fmetrics",
                "native metric scoring failed, falling back to the VapourSynth plugin",
            ] {
                assert!(
                    output.contains(expected),
                    "missing {expected:?} in {output:?}"
                );
            }
        }
    }
}

#[test]
fn fallback_quality_check_logs_metadata_cause_chain() {
    let metric = QualityMetric::default();
    assert_causes_emitted("quality_check", |error| {
        crate::core::sequence::quality_check::log_native_fallback(error, Engine::Fmetrics, &metric);
    });
}

#[test]
fn fallback_target_quality_logs_metadata_cause_chain() {
    let metric = QualityMetric::default();
    assert_causes_emitted("target_quality", |error| {
        crate::core::sequence::target_quality::log_native_fallback(
            error,
            Some(Engine::Fmetrics),
            &metric,
        );
    });
}
