//! Tests for `av1an --version`.
//!
//! `--version` prints the three "installed" tables — VapourSynth plugins,
//! encoders, and quality metric libraries — and the last of those is what
//! these cover. Its rows depend entirely on the machine: a library is either
//! loadable or it is not, and CI runners generally have none of them. The
//! assertions are therefore about the *report's* shape, not about any
//! particular library being present, and only the rows for libraries that
//! actually loaded are checked for details.

use crate::common::condor_cmd;
use assert_cmd::Command;

/// Run `av1an --version`, plus any extra arguments, and return its stdout.
///
/// # Panics
///
/// If the command fails, including when VapourSynth cannot build a core.
fn version_output(extra_args: &[&str]) -> String {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let mut cmd: Command = condor_cmd(&temp);
    let assert = cmd.arg("--version").args(extra_args).assert().success();

    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

/// Strip ANSI escapes so the rows can be matched as plain text.
///
/// The markers are deliberately coloured, which would otherwise break every
/// substring match on a row's name.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }

        // The CSI introducer is consumed first: `[` is 0x5B, which is itself
        // inside the 0x40..=0x7E final-byte range below, so treating it as a
        // terminator would leave the rest of the sequence behind.
        if chars.next() != Some('[') {
            continue;
        }
        for c in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&c) {
                break;
            }
        }
    }

    out
}

/// Whether `name` appears in the output with a marker showing a loaded library.
fn row_reports_available(output: &str, name: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim().starts_with('\u{2713}') && line.contains(name))
}

/// Whether `name` appears in the output with a marker showing a missing
/// library.
fn row_reports_absent(output: &str, name: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim().starts_with('\u{2717}') && line.contains(name))
}

/// Everything printed after the metric library table's header.
fn metric_library_section(output: &str) -> &str {
    output
        .split("Quality Metric Libraries")
        .nth(1)
        .expect("the metric library section should be printed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_reports_av1an_usage_identity() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let help = condor_cmd(&temp).arg("--help").assert().success();
        let help = String::from_utf8_lossy(&help.get_output().stdout);
        assert!(
            help.contains("Usage: av1an"),
            "unexpected CLI identity:\n{help}"
        );
    }

    #[test]
    fn version_output_starts_with_one_point_zero_point_zero() {
        let version = strip_ansi(&version_output(&[]));
        assert!(
            version.starts_with("1.0.0"),
            "unexpected CLI version:\n{version}"
        );
    }

    /// The header and one marker-bearing row for each metric library must
    /// appear, since which of these are present is exactly what a user is
    /// asking.
    #[test]
    fn reports_the_quality_metric_library_section() {
        let output = strip_ansi(&version_output(&[]));

        assert!(
            output.lines().any(|line| line.trim() == "Quality Metric Libraries"),
            "the metric library section should be printed, got:\n{output}"
        );

        for name in ["libvship", "fmetrics", "libvmaf"] {
            assert!(
                row_reports_available(&output, name) || row_reports_absent(&output, name),
                "{name} should be reported as either installed or missing, got:\n{output}"
            );
        }
    }

    /// An absent library must never carry a version or detail suffix, since
    /// there is nothing loaded to describe.
    #[test]
    fn absent_metric_library_rows_do_not_claim_a_version_or_details() {
        let output = strip_ansi(&version_output(&["--verbose"]));

        for line in metric_library_section(&output).lines() {
            let line = line.trim();
            let Some(name) = ["libvship", "fmetrics", "libvmaf"]
                .into_iter()
                .find(|name| line.starts_with(&format!("\u{2717} {name}")))
            else {
                continue;
            };

            assert_eq!(
                line,
                format!("\u{2717} {name}"),
                "{name} is reported missing, so it must not also claim a version: {line:?}"
            );
        }
    }

    /// Only loaded libraries have backend details to report.
    #[test]
    fn verbose_loaded_libvship_reports_backend_detail() {
        let output = strip_ansi(&version_output(&["--verbose"]));
        let section = metric_library_section(&output);

        if row_reports_available(&output, "libvship") {
            assert!(
                section.contains("backend:"),
                "a loaded libvship reports its compute backend under --verbose, got:\n{section}"
            );
        }
    }

    #[test]
    fn verbose_loaded_libvmaf_reports_cuda_support_detail() {
        let output = strip_ansi(&version_output(&["--verbose"]));
        let section = metric_library_section(&output);
        if row_reports_available(&output, "libvmaf") {
            assert!(
                section.contains("cuda"),
                "a loaded libvmaf reports whether CUDA is available, got:\n{section}"
            );
        }
    }

    /// Every device libvship enumerated must be listed with the index that
    /// selects it, since that index is the whole reason to print the table.
    ///
    /// The expected devices come from the library itself rather than from a
    /// fixture, so this asserts the report is complete on whichever machine
    /// runs it. That needs no new dependency: `andean-condor` is already a
    /// dev-time dependency and re-exports the same probe.
    #[test]
    fn verbose_lists_each_enumerated_libvship_device_index_and_name() {
        let output = strip_ansi(&version_output(&["--verbose"]));
        let section = metric_library_section(&output);

        let devices = andean_condor::metrics::libraries()
            .into_iter()
            .find(|info| info.name == "libvship")
            .map(|info| info.devices)
            .unwrap_or_default();

        if devices.is_empty() {
            // Nothing to report means nothing to assert; the absence of a
            // table is covered by `device_table_marks_one_default_or_none_without_devices`.
            return;
        }

        assert!(
            section.contains("Compute Devices"),
            "{} device(s) were enumerated, so they should be listed, got:\n{section}",
            devices.len()
        );

        for device in devices {
            assert!(
                section.contains(&format!("[{}]", device.id)),
                "device {} should be listed by the index that selects it, got:\n{section}",
                device.id
            );
            assert!(
                section.contains(&device.name),
                "device {} ({}) should be listed by name, got:\n{section}",
                device.id,
                device.name
            );
        }
    }

    /// Exactly one device may be marked as the default, and it must be one that
    /// is actually listed -- a default pointing at an absent device would send
    /// a user to configure something unusable.
    #[test]
    fn device_table_marks_one_default_or_none_without_devices() {
        let output = strip_ansi(&version_output(&["--verbose"]));
        let section = metric_library_section(&output);

        let defaults = section.lines().filter(|line| line.contains("(default)")).count();

        if row_reports_available(&output, "libvship") && section.contains("Compute Devices") {
            assert_eq!(
                defaults, 1,
                "a listed device table should mark one device as the default, got:\n{section}"
            );
        } else {
            assert_eq!(
                defaults, 0,
                "no devices are listed, so none may claim to be the default, got:\n{section}"
            );
        }
    }

    /// The VapourSynth plugin table must still be printed: the metric libraries
    /// are an addition to it, not a replacement.
    #[test]
    fn version_report_includes_vapoursynth_plugin_and_encoder_sections() {
        let output = strip_ansi(&version_output(&[]));

        for header in ["VapourSynth Plugins Installed", "Encoders Installed"] {
            assert!(
                output.contains(header),
                "{header} should still be printed, got:\n{output}"
            );
        }
    }
}
