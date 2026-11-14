use super::*;
use std::io::{BufWriter, Seek, SeekFrom};

fn count_record(count: u32) -> Result<u32> {
    count.checked_add(1).context("IVF record count exceeds u32")
}

fn read_header(path: &Path) -> Result<([u8; 32], Ratio<i64>)> {
    let mut header = [0; 32];
    File::open(path)
        .with_context(|| format!("opening IVF {}", path.display()))?
        .read_exact(&mut header)
        .with_context(|| format!("reading IVF header {}", path.display()))?;
    if &header[..4] != b"DKIF" || header[4..8] != [0, 0, 32, 0] {
        bail!(
            "{} requires a standard version 0, 32-byte IVF header",
            path.display()
        );
    }
    if !matches!(&header[8..12], b"AV01" | b"VP80" | b"VP90") {
        bail!("{} has an unsupported IVF codec", path.display());
    }
    if header[12..14] == [0, 0] || header[14..16] == [0, 0] {
        bail!("{} has invalid zero IVF dimensions", path.display());
    }
    let rate = u32::from_le_bytes(header[16..20].try_into().expect("4 bytes"));
    let scale = u32::from_le_bytes(header[20..24].try_into().expect("4 bytes"));
    if rate == 0 || scale == 0 {
        bail!("{} has an invalid zero IVF rate/scale", path.display());
    }
    let timebase = Ratio::new(i64::from(scale), i64::from(rate));
    Ok((header, timebase))
}

impl SceneConcatenator {
    /// Concatenates IVF parts using the input/timestamp policy of [`Self::ivf_cancel`].
    /// Scratch writer: the caller must exclusively own `output`; this can overwrite it.
    /// Use the final Sequence boundary for saved overwrite/source protection and publication.
    #[inline]
    pub fn ivf(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        Self::ivf_cancel(
            output,
            scene_paths,
            progress_tx,
            &Cancellation::from(Arc::clone(cancelled)),
        )
    }

    /// Concatenates nonempty standard version-0/32-byte-header IVF parts with matching
    /// codec/dimensions and equal rational timebases (nonzero u32 rate/scale).
    /// Input counts are hints: zero means unknown; output counts actual records, including
    /// invisible frames, not decoded visible frames. The total must fit u32.
    /// Timestamps must be nondecreasing; all timestamp arithmetic is checked u64.
    /// Equal PTS and gaps are preserved; each origin is removed once. The next part starts
    /// one nominal tick (scale/rate seconds) after the previous part's final record.
    /// EOF is valid only between complete records at actual input EOF. Framing is checked,
    /// not codec decodability; opaque payloads stream in bounded chunks with force checks.
    /// The first input's exact rate/scale pair is preserved; output counts actual records.
    /// Caller must exclusively own the scratch output (it may be truncated on error).
    #[inline]
    pub fn ivf_cancel(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<()> {
        cancellation.check_admission()?;
        Self::validate_scene_paths(scene_paths)?;
        let (first_header, timebase) = read_header(&scene_paths[0])?;
        for path in &scene_paths[1..] {
            let (header, tb) = read_header(path)?;
            if header[8..16] != first_header[8..16] || tb != timebase {
                bail!(
                    "{} has an incompatible IVF codec, dimensions or timebase",
                    path.display()
                );
            }
        }
        let mut partial = InterruptedOutput::new(output, cancellation);
        let mut writer = BufWriter::new(
            File::create(output)
                .with_context(|| format!("creating IVF output {}", output.display()))?,
        );
        let mut output_header = first_header;
        output_header[24..32].fill(0); // Unknown count until all records are copied; reserved bytes zero.
        writer.write_all(&output_header).context("writing IVF output header")?;

        let total_scenes = scene_paths.len();
        let mut timestamp_offset = 0u64;
        let mut record_count = 0u32;
        // SECURITY: Never allocate the untrusted u32 payload size (up to 4 GiB).
        let mut buffer = [0; 8192];
        for (index, file) in scene_paths.iter().enumerate() {
            cancellation.check_forced()?;

            Self::send_progress(
                progress_tx,
                (index + 1) as f64 / total_scenes as f64 * 100.0,
            );

            let mut origin = None;
            let mut previous_timestamp = None;
            let mut last_local = 0u64;
            let mut input = BufReader::new(
                File::open(file).with_context(|| format!("opening IVF {}", file.display()))?,
            );
            input
                .seek(SeekFrom::Start(32))
                .with_context(|| format!("seeking IVF records in {}", file.display()))?;
            let mut part_records = 0u32;

            loop {
                cancellation.check_forced()?;
                // fill_buf refills an exhausted buffer; only actual EOF ends a part.
                if input
                    .fill_buf()
                    .with_context(|| {
                        format!(
                            "reading IVF record {part_records} header in {}",
                            file.display()
                        )
                    })?
                    .is_empty()
                {
                    break;
                }
                let mut frame_header = [0; 12];
                input.read_exact(&mut frame_header).with_context(|| {
                    format!(
                        "reading IVF record {part_records} header in {}",
                        file.display()
                    )
                })?;
                let mut remaining =
                    u32::from_le_bytes(frame_header[..4].try_into().expect("4 bytes"));
                let timestamp =
                    u64::from_le_bytes(frame_header[4..12].try_into().expect("8 bytes"));
                if previous_timestamp.is_some_and(|previous| timestamp < previous) {
                    bail!(
                        "{} IVF timestamp decreases at record {part_records}",
                        file.display()
                    );
                }
                previous_timestamp = Some(timestamp);
                last_local =
                    timestamp.checked_sub(*origin.get_or_insert(timestamp)).with_context(|| {
                        format!(
                            "{} IVF timestamp underflow at record {part_records}",
                            file.display()
                        )
                    })?;
                let appended = last_local.checked_add(timestamp_offset).with_context(|| {
                    format!(
                        "{} IVF timestamp offset overflow at record {part_records}",
                        file.display()
                    )
                })?;
                record_count = count_record(record_count).with_context(|| {
                    format!("counting IVF record {part_records} in {}", file.display())
                })?;
                frame_header[4..12].copy_from_slice(&appended.to_le_bytes());
                writer.write_all(&frame_header).with_context(|| {
                    format!(
                        "writing IVF record {part_records} header from {}",
                        file.display()
                    )
                })?;
                while remaining > 0 {
                    cancellation.check_forced()?;
                    let length = remaining.min(buffer.len() as u32) as usize;
                    input.read_exact(&mut buffer[..length]).with_context(|| {
                        format!(
                            "reading IVF record {part_records} payload in {}",
                            file.display()
                        )
                    })?;
                    writer.write_all(&buffer[..length]).with_context(|| {
                        format!(
                            "writing IVF record {part_records} payload from {}",
                            file.display()
                        )
                    })?;
                    remaining -= length as u32;
                    #[cfg(test)]
                    super::tests::publication::during_payload(output, cancellation);
                }
                // Bounded by the already checked global record count.
                part_records += 1;
            }
            if origin.is_none() {
                bail!("{} has no IVF records", file.display());
            }
            if index + 1 < total_scenes {
                // IVF ticks are scale/rate seconds; retain gaps, with one nominal tick
                // after the final frame. Do not add any part's absolute origin twice.
                timestamp_offset = timestamp_offset
                    .checked_add(last_local)
                    .and_then(|end| end.checked_add(1))
                    .with_context(|| format!("{} IVF timestamp end overflow", file.display()))?;
            }
        }

        writer.seek(SeekFrom::Start(24)).context("seeking IVF output record count")?;
        writer
            .write_all(&record_count.to_le_bytes())
            .context("writing IVF output record count")?;
        writer.flush().context("flushing IVF output record count")?;
        drop(writer);
        partial.complete()
    }
}

#[cfg(test)]
mod tests {
    use super::count_record;

    #[test]
    fn actual_record_count_rejects_overflow() {
        assert_eq!(
            count_record(u32::MAX - 1).expect("last representable record"),
            u32::MAX
        );
        assert!(
            count_record(u32::MAX)
                .expect_err("record count overflow")
                .to_string()
                .contains("record count")
        );
    }
}
