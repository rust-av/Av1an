//! In-memory typed-error composition; native streaming cases live in tests/core.
use super::*;
use crate::{
    core::encoder::{EncoderProcessError, EncoderResult},
    models::encoder::EncoderBase,
};

fn result_in(error: &anyhow::Error) -> Option<&EncoderResult> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<EncoderResult>())
        .or_else(|| match error.downcast_ref::<ParallelEncoderError>() {
            Some(ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
            _ => None,
        })
}

#[test]
fn joined_stream_errors_keep_forced_or_decoder_priority_and_typed_codec_diagnostics() {
    use crate::utils::cancellation::Cancelled;
    use std::{io, os::unix::process::ExitStatusExt};
    let codec = || EncoderResult {
        encoder: EncoderBase::X264,
        parameters: vec!["--bad-flag".into()],
        status: std::process::ExitStatus::from_raw(42 << 8),
        stdout: b"stdout\xff".to_vec().into(),
        stderr: "stream failures codec cause".to_owned().into(),
    };
    let encoded = || {
        anyhow::Error::from(ParallelEncoderError::EncoderFailed {
            scene: 7,
            result: codec(),
        })
    };
    let decode = || {
        anyhow::Error::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream failures decoder cause",
        ))
        .context("Scene 7 frame production")
    };
    let disconnected = || {
        anyhow::Error::from(crossbeam_channel::SendError(std::io::Cursor::new(vec![
            0_u8,
        ])))
    };
    let merged = streaming::joined_scene_result(Err(encoded()), Err(disconnected()))
        .expect_err("codec primary");
    assert!(merged.downcast_ref::<ParallelEncoderError>().is_some());
    assert_eq!(
        result_in(&merged).expect("typed codec").status.code(),
        Some(42)
    );
    assert!(!format!("{merged:#}").contains("sending on a disconnected channel"));
    let merged =
        streaming::joined_scene_result(Err(encoded()), Err(decode())).expect_err("decode primary");
    assert_eq!(
        merged.downcast_ref::<io::Error>().expect("original typed decoder cause").kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        merged.root_cause().to_string(),
        "stream failures decoder cause"
    );
    let text = format!("{merged:#}");
    assert!(text.starts_with("Scene 7 frame production"), "{text}");
    assert!(
        text.contains("stream failures decoder cause")
            && text.contains("stream failures codec cause"),
        "{text}"
    );
    assert!(text.contains("Scene 7") && text.contains("42"), "{text}");
    let forced = anyhow::Error::from(codec()).context(Cancelled::Forced);
    let baseline = [format!("{forced:#}"), format!("{forced:?}")];
    let merged =
        streaming::joined_scene_result(Err(forced), Err(decode())).expect_err("force primary");
    assert_eq!(merged.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
    assert_eq!(
        result_in(&merged).expect("original forced diagnostic").status.code(),
        Some(42)
    );
    assert!(merged.to_string().contains("stream failures decoder cause"));
    for (baseline, rendered) in
        baseline.into_iter().zip([format!("{merged:#}"), format!("{merged:?}")])
    {
        assert_eq!(
            rendered.matches("stream failures codec cause").count(),
            baseline.matches("stream failures codec cause").count(),
            "forced primary is not copied into context: {rendered}"
        );
        assert!(
            rendered.contains("Scene 7 frame production")
                && rendered.contains("stream failures decoder cause"),
            "genuine producer failure retained under force: {rendered}"
        );
    }
    let merged = streaming::joined_scene_result(Ok(codec()), Err(decode()))
        .expect_err("source without consumer failure");
    assert!(merged.is::<io::Error>());
    assert!(
        streaming::joined_scene_result(Err(encoded()), Ok(()))
            .expect_err("codec alone")
            .is::<ParallelEncoderError>()
    );
    assert!(streaming::joined_scene_result(Ok(codec()), Ok(())).is_ok());
}

#[test]
fn induced_send_and_cancel_fallout_does_not_duplicate_consumer_error_rendering() {
    use crate::utils::cancellation::Cancelled;
    use std::{io, os::unix::process::ExitStatusExt};
    let mut differences = Vec::new();
    for kind in ["send", "graceful", "forced"] {
        let producer = match kind {
            "send" => {
                anyhow::Error::from(crossbeam_channel::SendError(std::io::Cursor::new(vec![
                    0_u8,
                ])))
            },
            "graceful" => Cancelled::Graceful.into(),
            _ => Cancelled::Forced.into(),
        }
        .context("Scene 7 frame production");
        let codec = EncoderResult {
            encoder: EncoderBase::X264,
            parameters: vec!["--bad-flag".into()],
            status: std::process::ExitStatus::from_raw(42 << 8),
            stdout: "stream failures cause-chain unique stdout marker".to_owned().into(),
            stderr: "stream failures cause-chain unique stderr marker".to_owned().into(),
        };
        let consumer = anyhow::Error::from(EncoderProcessError {
            stdout: codec.stdout.clone(),
            stderr: codec.stderr.clone(),
            result: Some(codec),
            failures: vec![
                anyhow::Error::from(io::Error::from(io::ErrorKind::BrokenPipe))
                    .context("Writing encoder STDIN"),
            ],
        })
        .context("Scene 7 consumer");
        let baseline = [format!("{consumer:#}"), format!("{consumer:?}")];
        let original_root = consumer.root_cause().to_string();
        let merged = streaming::joined_scene_result(Err(consumer), Err(producer))
            .expect_err("original consumer failure");
        assert!(merged.is::<EncoderProcessError>());
        assert_eq!(
            result_in(&merged).expect("typed codec").status.code(),
            Some(42)
        );
        assert_eq!(merged.root_cause().to_string(), original_root);
        for ((style, baseline), rendered) in ["alternate", "Debug"]
            .into_iter()
            .zip(baseline)
            .zip([format!("{merged:#}"), format!("{merged:?}")])
        {
            for marker in [
                "stream failures cause-chain unique stdout marker",
                "stream failures cause-chain unique stderr marker",
            ] {
                let before = baseline.matches(marker).count();
                let after = rendered.matches(marker).count();
                assert!(before > 0, "non-vacuous consumer baseline");
                if after != before {
                    differences.push(format!(
                        "{kind}/{style}: {marker}: baseline={before}, joined={after}"
                    ));
                }
            }
            if rendered != baseline {
                differences.push(format!(
                    "{kind}/{style}: fallout changed original consumer rendering"
                ));
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn genuine_codec_failure_is_attached_once_without_duplicating_the_decoder_root_cause() {
    use std::{io, os::unix::process::ExitStatusExt};
    let producer = anyhow::Error::from(io::Error::new(
        io::ErrorKind::InvalidData,
        "stream failures cause-chain unique inner decode cause",
    ))
    .context("stream failures cause-chain decode operation")
    .context("Scene 7 frame production");
    let baseline = [format!("{producer:#}"), format!("{producer:?}")];
    let consumer = anyhow::Error::from(EncoderResult {
        encoder: EncoderBase::X264,
        parameters: vec![],
        status: std::process::ExitStatus::from_raw(42 << 8),
        stdout: "stream failures cause-chain secondary stdout marker".to_owned().into(),
        stderr: "stream failures cause-chain secondary stderr marker".to_owned().into(),
    })
    .context("Scene 7 consumer");
    let merged = streaming::joined_scene_result(Err(consumer), Err(producer))
        .expect_err("genuine double failure");
    assert_eq!(
        merged.downcast_ref::<io::Error>().expect("original typed source").kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        merged.root_cause().to_string(),
        "stream failures cause-chain unique inner decode cause"
    );
    assert!(merged.to_string().starts_with("Scene 7 frame production"));
    let mut differences = Vec::new();
    for ((style, baseline), rendered) in ["alternate", "Debug"]
        .into_iter()
        .zip(baseline)
        .zip([format!("{merged:#}"), format!("{merged:?}")])
    {
        for marker in [
            "stream failures cause-chain secondary stdout marker",
            "stream failures cause-chain secondary stderr marker",
            "Scene 7 consumer",
            "stream failures cause-chain decode operation",
        ] {
            assert!(
                rendered.contains(marker),
                "{style}: both genuine diagnostics returned: {rendered}"
            );
        }
        let before = baseline
            .matches("stream failures cause-chain unique inner decode cause")
            .count();
        let after = rendered
            .matches("stream failures cause-chain unique inner decode cause")
            .count();
        assert_eq!(before, 1, "literal source cause baseline");
        if after != before {
            differences.push(format!(
                "{style}: inner decode cause baseline={before}, joined={after}"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
