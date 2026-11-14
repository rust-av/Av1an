#[cfg(test)]
mod filter_policy {
    use std::env;

    use california_condor::logging::init_logging;
    use tracing_subscriber::filter::LevelFilter;

    fn check_logging(phase: &str, overrides: bool, test: &str) -> anyhow::Result<()> {
        assert!(["policy", "duplicates", "flush"].contains(&phase));
        if let Some(root) = env::var_os("LOGGING_FILTER_CHILD") {
            assert_eq!(env::var("LOGGING_FILTER_PHASE")?, phase);
            assert_eq!(env::var("LOGGING_FILTER_OVERRIDES")?, overrides.to_string());
            let root = std::path::PathBuf::from(root);
            let _guard = init_logging(
                LevelFilter::INFO,
                &root.join("events.log"),
                LevelFilter::DEBUG,
            )?;
            tracing::debug!(target: "california_condor", "TUI_LOGGING-debug-policy");
            tracing::info!(target: "andean_condor", "TUI_LOGGING-info-policy");
            tracing::warn!(target: "andean_condor", "TUI_LOGGING-warn-policy");
            tracing::info!(target: "foreign_target", "TUI_LOGGING-foreign-policy");
            if phase == "duplicates" {
                #[cfg(target_os = "linux")]
                let threads = std::fs::read_dir("/proc/self/task")?.count();
                for _ in 0..3 {
                    let error = init_logging(
                        LevelFilter::INFO,
                        &root.join("rejected.log"),
                        LevelFilter::DEBUG,
                    )
                    .expect_err("duplicate subscriber must return an error, not leak or panic");
                    assert!(error.to_string().contains("global default subscriber"));
                }
                #[cfg(target_os = "linux")]
                {
                    // WorkerGuard acknowledges shutdown before its OS thread exits.
                    // Keep the final log after teardown so this cannot mask a missing flush.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while std::fs::read_dir("/proc/self/task")?.count() != threads {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "rejected setup leaked a logging worker"
                        );
                        std::thread::yield_now();
                    }
                    println!("logging duplicate setup: Linux workers retired");
                }
                println!("logging duplicate setup: 3 rejections completed");
            }
            tracing::error!(target: "california_condor", "TUI_LOGGING-policy-final");
            println!("logging {phase}/{overrides}: child assertions completed");
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let mut child = std::process::Command::new(env::current_exe()?);
        child
            .args(["--exact", test, "--nocapture", "--quiet"])
            .env("LOGGING_FILTER_CHILD", root.path())
            .env("LOGGING_FILTER_PHASE", phase)
            .env("LOGGING_FILTER_OVERRIDES", overrides.to_string())
            .env_remove("RUST_LOG");
        if overrides {
            child.env("RUST_LOG", "andean_condor=warn,california_condor=debug");
        }
        let output = child.output()?;
        assert!(output.status.success(), "{output:?}");
        let console = String::from_utf8(output.stderr)?;
        let stdout = String::from_utf8(output.stdout)?;
        assert!(
            stdout.contains("1 passed; 0 failed;"),
            "exactly one logging case ran: {stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "logging {phase}/{overrides}: child assertions completed"
            )),
            "selected logging body completed: {stdout}"
        );
        if phase == "duplicates" {
            assert!(stdout.contains("logging duplicate setup: 3 rejections completed"));
            #[cfg(target_os = "linux")]
            assert!(stdout.contains("logging duplicate setup: Linux workers retired"));
        }
        let file = std::fs::read_to_string(root.path().join("events.log"))?;
        assert!(
            !stdout.contains("TUI_LOGGING-"),
            "logs must never use stdout"
        );
        assert!(!console.contains('\x1b') && !file.contains('\x1b'));
        if phase != "flush" {
            assert_eq!(console.contains("TUI_LOGGING-debug-policy"), overrides);
            assert_eq!(console.contains("TUI_LOGGING-info-policy"), !overrides);
            assert_eq!(file.contains("TUI_LOGGING-info-policy"), !overrides);
            assert!(
                file.contains("TUI_LOGGING-debug-policy") && file.contains("california_condor")
            );
        }
        for log in [&console, &file] {
            assert!(
                log.contains("TUI_LOGGING-warn-policy") && log.contains("TUI_LOGGING-policy-final")
            );
            assert!(!log.contains("TUI_LOGGING-foreign-policy"));
        }
        eprintln!("{stdout}");
        Ok(())
    }

    #[test]
    fn default_filters_keep_console_info_file_debug_and_exclude_foreign_targets()
    -> anyhow::Result<()> {
        check_logging(
            "policy",
            false,
            "filter_policy::default_filters_keep_console_info_file_debug_and_exclude_foreign_targets",
        )
    }

    #[test]
    fn rust_log_overrides_console_and_file_target_filters() -> anyhow::Result<()> {
        check_logging(
            "policy",
            true,
            "filter_policy::rust_log_overrides_console_and_file_target_filters",
        )
    }

    #[test]
    fn duplicate_logging_setup_returns_errors_and_retires_linux_workers() -> anyhow::Result<()> {
        if env::var_os("LOGGING_FILTER_CHILD").is_some() {
            return check_logging(
                "duplicates",
                env::var("LOGGING_FILTER_OVERRIDES")?.parse()?,
                "filter_policy::duplicate_logging_setup_returns_errors_and_retires_linux_workers",
            );
        }
        for overrides in [false, true] {
            check_logging(
                "duplicates",
                overrides,
                "filter_policy::duplicate_logging_setup_returns_errors_and_retires_linux_workers",
            )?;
        }
        Ok(())
    }

    #[test]
    fn logging_guard_drop_flushes_final_message_without_stdout_or_ansi_pollution()
    -> anyhow::Result<()> {
        if env::var_os("LOGGING_FILTER_CHILD").is_some() {
            return check_logging(
                "flush",
                env::var("LOGGING_FILTER_OVERRIDES")?.parse()?,
                "filter_policy::logging_guard_drop_flushes_final_message_without_stdout_or_ansi_pollution",
            );
        }
        for overrides in [false, true] {
            check_logging(
                "flush",
                overrides,
                "filter_policy::logging_guard_drop_flushes_final_message_without_stdout_or_ansi_pollution",
            )?;
        }
        Ok(())
    }
}
