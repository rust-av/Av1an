use andean_condor::core::encoder::EncodeResourceUsage;

/// Human-readable CPU and resident memory of a measured process, or `None`
/// when nothing was sampled: both fields default to zero.
pub(crate) fn describe(usage: EncodeResourceUsage) -> Option<String> {
    let mut parts = Vec::new();
    if usage.cpu > 0.0 {
        parts.push(format!("CPU {:.0}%", usage.cpu));
    }
    if usage.memory > 0 {
        let mib = usage.mib();
        parts.push(if mib >= 1024 {
            format!("{:.1} GiB", mib as f64 / 1024.0)
        } else {
            format!("{mib} MiB")
        });
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use andean_condor::core::encoder::EncodeResourceUsage;

    use super::describe;

    const MIB: u64 = 1024 * 1024;

    fn sampled(cpu: f32, memory: u64) -> EncodeResourceUsage {
        EncodeResourceUsage { cpu, memory }
    }

    /// Worked examples: percent is rounded for display, memory switches to GiB
    /// where that reads better, and an unsampled process has no description.
    #[test]
    fn usage_reads_as_percent_and_memory_and_is_hidden_when_unsampled() {
        assert_eq!(describe(sampled(0.0, 0)), None);
        assert_eq!(
            describe(sampled(412.0, 1536 * MIB)).as_deref(),
            Some("CPU 412% · 1.5 GiB")
        );
        assert_eq!(
            describe(sampled(99.4, 200 * MIB)).as_deref(),
            Some("CPU 99% · 200 MiB")
        );
        assert_eq!(
            describe(sampled(0.0, 200 * MIB)).as_deref(),
            Some("200 MiB")
        );
        assert_eq!(describe(sampled(50.0, 0)).as_deref(), Some("CPU 50%"));
    }
}
