use super::*;
use andean_condor::core::input::Input;
use std::{
    collections::HashMap, os::unix::fs::PermissionsExt, path::PathBuf, sync::mpsc, time::Duration,
};

#[test]
fn invalid_pass_settings_fail_before_spawn_or_stream_receive() {
    preflight_cases(
        "invalid_pass_settings_fail_before_spawn_or_stream_receive",
        &["input", "input-pass", "stream", "stream-pass", "tuple-input", "tuple-stream"],
    );
}

#[test]
fn vvenc_frontend_pass_conflict_fails_before_encoding() {
    preflight_cases(
        "vvenc_frontend_pass_conflict_fails_before_encoding",
        &["vvenc-stream"],
    );
}

fn preflight_cases(test: &str, routes: &[&str]) {
    if std::env::var_os("ENCODER_PREFLIGHT_DIR").is_some() {
        let route = std::env::var("ENCODER_PREFLIGHT_ROUTE").expect("route");
        assert!(
            routes.contains(&route.as_str()),
            "selected preflight route belongs to this contract"
        );
        assert_preflight_before_spawn_or_receive();
        println!("encoder preflight {route}: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let fake = temp.path().join("encoder");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf spawned > \"$ENCODER_PREFLIGHT_DIR/spawned\"\nexit 42\n",
    )
    .expect("fake");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("executable");
    std::fs::write(temp.path().join("blank.vpy"), "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("script");
    for route in routes {
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", &format!("core::encoder::preflight::{test}"), "--nocapture"])
            .env("ENCODER_PREFLIGHT_DIR", temp.path())
            .env("ENCODER_PREFLIGHT_ROUTE", route)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("watchdog child");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait") {
                break status;
            }
            if start.elapsed() > Duration::from_secs(5) {
                child.kill().expect("kill blocked child");
                child.wait().expect("reap blocked child");
                panic!("{route} blocked before validation");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let out = child.wait_with_output().expect("child output");
        let log = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            status.success()
                && std::fs::read_to_string(temp.path().join("preflight-selected"))
                    .is_ok_and(|selected| selected == *route),
            "{route}: child failed or was not selected"
        );
        assert!(
            log.contains("1 passed; 0 failed")
                && log.contains(&format!("encoder preflight {route}: assertions completed")),
            "selected preflight assertions executed: {log}"
        );
        assert!(
            !temp.path().join("spawned").exists(),
            "{route}: encoder spawned"
        );
        assert!(
            !temp.path().join("out.ivf").exists(),
            "{route}: output created"
        );
        print!("{log}");
    }
}

fn assert_preflight_before_spawn_or_receive() {
    let dir = PathBuf::from(std::env::var_os("ENCODER_PREFLIGHT_DIR").expect("fixture directory"));
    let route = std::env::var("ENCODER_PREFLIGHT_ROUTE").expect("route");
    std::fs::write(dir.join("preflight-selected"), &route).expect("selection handshake");
    let mut encoder = Encoder::default_from_base(
        &if route == "vvenc-stream" {
            EncoderBase::VVenC
        } else {
            EncoderBase::AOM
        },
        false,
    );
    encoder.set_executable(Some(dir.join("encoder")));
    let pass = if route.starts_with("tuple") {
        (3, 3)
    } else {
        (1, 1)
    };
    *encoder.passes_mut().expect("passes") =
        EncoderPasses::All(if route.starts_with("tuple") { 1 } else { 0 });
    if route == "vvenc-stream" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("passes", 1.0), ("bitrate", 1000.0)],
        ));
    }
    let (tx, _rx) = mpsc::channel();
    let output = dir.join("out.ivf");
    let result = if route.contains("input") {
        let model = andean_condor::models::input::Input::VapourSynthScript {
            source: andean_condor::models::input::VapourSynthScriptSource::Path(
                dir.join("blank.vpy"),
            ),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let mut input = Input::from_data(&model).expect("blank input");
        if route == "input" {
            encoder.encode_with_input(&mut input, &output, 0, 1, tx)
        } else {
            encoder.encode_pass_with_input(&mut input, &output, 0, 1, pass, tx, None)
        }
    } else {
        let (_sender, receiver) = crossbeam_channel::unbounded();
        // Keep the sender alive: a misplaced receive must trip the outer watchdog.
        let result = if route == "stream" || route == "vvenc-stream" {
            encoder.encode_with_stream(receiver, &output, tx, &super::clip_info())
        } else {
            encoder.encode_pass_with_stream(pass, receiver, &output, tx, &super::clip_info(), None)
        };
        drop(_sender);
        result
    };
    let error = result.expect_err("invalid settings must fail before encoding");
    if route == "vvenc-stream" {
        use andean_condor::models::encoder::EncoderSettingsError;
        assert!(
            matches!(error.downcast_ref::<EncoderSettingsError>(), Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("frontend")),
            "{error:#}"
        );
        let error = encoder
            .parameters_with_output((1, 2), &output, &super::clip_info())
            .expect_err("first-pass argv must reject the whole-job conflict");
        assert!(
            matches!(
                error.downcast_ref::<EncoderSettingsError>(),
                Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, _))
            ),
            "{error:#}"
        );
        return;
    }
    assert!(error.to_string().contains("pass"), "{error:#}");
}
