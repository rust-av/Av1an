use super::*;

impl SceneConcatenator {
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn mkvmerge(
        scenes_directory: &Path,
        output: &Path,
        scene_paths: &[PathBuf],
        input: Option<&Path>,
        duration: Ratio<i64>,
        config: Option<&MkvmergeConfig>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        #[cfg(windows)]
        const MAXIMUM_CHUNKS_PER_MERGE: usize = usize::MAX;
        #[cfg(not(windows))]
        const MAXIMUM_CHUNKS_PER_MERGE: usize = 512;

        // mkvmerge does not accept UNC paths on Windows
        #[cfg(windows)]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            const UNC_PREFIX: &str = r#"\\?\"#;

            let p = p.as_ref().display().to_string();
            p.strip_prefix(UNC_PREFIX).map_or_else(
                || p.clone(),
                |path| {
                    path.strip_prefix("UNC")
                        .map_or_else(|| path.to_string(), |p2| format!("\\{p2}"))
                },
            )
        }

        #[cfg(not(windows))]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            p.as_ref().display().to_string()
        }

        let scratch_directory = Self::scratch_directory(scenes_directory);
        if !scratch_directory.exists() {
            std::fs::create_dir_all(&scratch_directory)?;
        }
        let options_path = scratch_directory.join("options.json");
        let fixed_output = fix_path(output);
        let fixed_input = input.map(fix_path);
        let fixed_extra_inputs = config
            .map(|config| {
                config
                    .extra_inputs
                    .iter()
                    .map(|extra| (fix_path(&extra.path), extra))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let tags_xml_path = match config.and_then(|config| config.metadata.as_ref()) {
            Some(metadata) => {
                let path = scratch_directory.join("global-tags.xml");
                std::fs::write(&path, tags_xml(metadata))?;
                Some(fix_path(&path))
            },
            None => None,
        };

        let chunk_groups: Vec<Vec<PathBuf>> = scene_paths
            .chunks(MAXIMUM_CHUNKS_PER_MERGE)
            .map(|chunk| chunk.to_vec())
            .collect();

        if chunk_groups.len() == 1 {
            // Intermediate groups are unnecessary
            let args = mkvmerge_args(
                &fixed_output,
                &scene_paths.iter().map(fix_path).collect::<Vec<_>>(),
                fixed_input.as_deref(),
                Some(duration),
                config,
                &fixed_extra_inputs,
                tags_xml_path.as_deref(),
            );
            write_options(&args, &options_path)?;
        } else {
            for (group_index, chunk_group) in chunk_groups.iter().enumerate() {
                if cancelled.load(sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }

                let group_options_path = scratch_directory.join(format!("{group_index:05}.json"));
                let group_output_path =
                    fix_path(scratch_directory.join(format!("{group_index:05}.mkv")));

                let group_args = mkvmerge_args(
                    &group_output_path,
                    &chunk_group.iter().map(fix_path).collect::<Vec<_>>(),
                    None,
                    None,
                    None,
                    &[],
                    None,
                );
                write_options(&group_args, &group_options_path)?;

                // Paths in the options files are relative to this process's working
                // directory, so mkvmerge must run from it too.
                let mut group_cmd = Command::new("mkvmerge");
                group_cmd.arg(format!("@{}", fix_path(&group_options_path)));
                group_cmd.stdin(Stdio::null());
                group_cmd.stdout(Stdio::piped());
                group_cmd.stderr(Stdio::piped());

                let mut group_child =
                    group_cmd.spawn().with_context(|| "Failed to concatenate with mkvmerge")?;
                let group_stdout = group_child.stdout.take().expect("mkvmerge should have STDOUT");
                let group_stderr = group_child.stderr.take().expect("mkvmerge should have STDERR");

                let group_stderr_output = Arc::new(Mutex::new(String::new()));
                let group_stderr_clone = Arc::clone(&group_stderr_output);
                let group_stderr_thread = thread::spawn(move || -> Result<()> {
                    let mut reader = BufReader::new(group_stderr);
                    let mut buf = Vec::with_capacity(256);
                    loop {
                        match reader.read_until(b'\n', &mut buf) {
                            Ok(0) => break,
                            Ok(_) => {
                                if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                                    group_stderr_clone
                                        .lock()
                                        .expect("mutex should acquire lock")
                                        .push_str(line);
                                }
                                buf.clear();
                            },
                            Err(e) => return Err(e.into()),
                        }
                    }
                    Ok(())
                });

                let (cancelled, group_stdout_output) = Self::stream_mkvmerge_progress(
                    group_stdout,
                    progress_tx,
                    Some((group_index, chunk_groups.len())),
                    cancelled,
                )?;
                if cancelled {
                    group_child.kill()?;
                    group_child.wait()?;
                    return Ok(());
                }

                let group_status =
                    group_child.wait().with_context(|| "Failed to wait for mkvmerge")?;
                group_stderr_thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("mkvmerge STDERR thread panicked"))??;
                if !group_status.success() {
                    let error = SceneConcatenatorError::MkvmergeFailed {
                        status: group_status,
                        stdout: group_stdout_output,
                        stderr: group_stderr_output
                            .lock()
                            .expect("mutex should acquire lock")
                            .clone(),
                    };
                    error!("{}", error);
                    bail!(error);
                }
            }

            let chunk_group_options_names = chunk_groups
                .iter()
                .enumerate()
                .map(|(index, _)| fix_path(scratch_directory.join(format!("{index:05}.mkv"))))
                .collect::<Vec<_>>();
            let args = mkvmerge_args(
                &fixed_output,
                &chunk_group_options_names,
                fixed_input.as_deref(),
                Some(duration),
                config,
                &fixed_extra_inputs,
                tags_xml_path.as_deref(),
            );
            write_options(&args, &options_path)?;
        }

        let mut cmd = Command::new("mkvmerge");
        cmd.arg(format!("@{}", fix_path(options_path)));
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| "Failed to spawn mkvmerge")?;
        let stdout = child.stdout.take().expect("mkvmerge should have STDOUT");
        let stderr = child.stderr.take().expect("mkvmerge should have STDERR");

        let stderr_output = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_output);
        let stderr_thread = thread::spawn(move || -> Result<()> {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::with_capacity(256);
            loop {
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                            stderr_clone.lock().expect("mutex should acquire lock").push_str(line);
                        }
                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });

        let (cancelled, stdout_output) =
            Self::stream_mkvmerge_progress(stdout, progress_tx, None, cancelled)?;
        if cancelled {
            child.kill()?;
            child.wait()?;
            return Ok(());
        }

        let status = child.wait().with_context(|| "Failed to wait for mkvmerge")?;
        stderr_thread
            .join()
            .map_err(|_| anyhow::anyhow!("mkvmerge STDERR thread panicked"))??;
        if !status.success() {
            let error = SceneConcatenatorError::MkvmergeFailed {
                status,
                stdout: stdout_output,
                stderr: stderr_output.lock().expect("mutex should acquire lock").clone(),
            };
            error!("{}", error);
            bail!(error);
        }

        Ok(())
    }

    /// Streams mkvmerge STDOUT, parse `Progress: N%`, and emit progress
    #[inline]
    fn stream_mkvmerge_progress(
        stdout: impl std::io::Read,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        group: Option<(usize, usize)>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(bool, String)> {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(256);
        let mut output = String::new();

        loop {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                return Ok((true, output));
            }

            match reader.read_until(b'\r', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                        output.push_str(line);
                        if let Some(percentage) = Self::parse_mkvmerge_progress(line) {
                            let percentage = match group {
                                Some((group_index, total_groups)) => {
                                    (group_index as f64 + percentage / 100.0) / total_groups as f64
                                        * 100.0
                                },
                                None => percentage,
                            };
                            Self::send_progress(progress_tx, percentage);
                        }
                    }
                    buf.clear();
                },
                Err(e) => return Err(e.into()),
            }
        }

        Ok((false, output))
    }

    /// Parses a single mkvmerge progress line (e.g. `Progress: 33%`)
    #[inline]
    fn parse_mkvmerge_progress(line: &str) -> Option<f64> {
        let line = line.trim();
        let progress = line.rfind("Progress: ")?;
        let (_, progress) = line.split_at(progress + "Progress: ".len());
        progress.trim().strip_suffix('%')?.trim().parse().ok()
    }
}

/// Writes an mkvmerge argument list to disk as the JSON array that mkvmerge
/// reads from an `@options` file.
fn write_options(args: &[String], path: &Path) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(serde_json::to_string_pretty(args)?.as_bytes())?;
    Ok(())
}

/// Builds the mkvmerge option-file argument list.
///
/// Option order is significant: per-file options apply to the next source file,
/// the encoded scenes are appended as one bracketed logical source, and the
/// video track options therefore precede the opening bracket.
#[allow(clippy::too_many_arguments)]
fn mkvmerge_args(
    output: &str,
    chunks: &[String],
    input: Option<&str>,
    duration: Option<Ratio<i64>>,
    config: Option<&MkvmergeConfig>,
    extra_inputs: &[(String, &MkvmergeExtraInput)],
    tags_xml: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["-o".to_owned(), output.to_owned()];

    if let Some(title) = config
        .and_then(|config| config.metadata.as_ref())
        .and_then(|metadata| metadata.title.as_ref())
    {
        args.push("--title".to_owned());
        args.push(title.clone());
    }
    if let Some(xml) = tags_xml {
        args.push("--global-tags".to_owned());
        args.push(xml.to_owned());
    }
    if let Some(chapters) = config.and_then(|config| config.chapters.as_ref()) {
        push_chapter_globals(&mut args, chapters);
    }
    if let Some(config) = config {
        args.extend(config.attachment_additions());
    }

    if let Some(input) = input {
        args.push("--no-video".to_owned());
        if let Some(config) = config {
            push_source_metadata_flags(
                &mut args,
                config.chapters.as_ref(),
                config.metadata.as_ref(),
            );
            args.extend(config.input_args());
        }
        args.push(input.to_owned());
    }

    for (path, extra) in extra_inputs {
        push_source_metadata_flags(&mut args, extra.chapters.as_ref(), extra.metadata.as_ref());
        args.extend(extra.input_args());
        args.push(path.clone());
    }

    if let Some(duration) = duration {
        args.push("--default-duration".to_owned());
        args.push(format!("0:{}/{}fps", duration.numer(), duration.denom()));
    }
    if let Some(config) = config {
        args.extend(config.video_args());
    }

    args.push("[".to_owned());
    args.extend(chunks.iter().cloned());
    args.push("]".to_owned());
    args
}

fn push_chapter_globals(args: &mut Vec<String>, chapters: &Chapters) {
    if let Some(language) = &chapters.language {
        args.push("--chapter-language".to_owned());
        args.push(language.clone());
    }
    if let Some(charset) = &chapters.charset {
        args.push("--chapter-charset".to_owned());
        args.push(charset.clone());
    }
}

fn push_source_metadata_flags(
    args: &mut Vec<String>,
    chapters: Option<&Chapters>,
    metadata: Option<&Metadata>,
) {
    if chapters.is_some_and(|chapters| !chapters.copy) {
        args.push("--no-chapters".to_owned());
    }
    if metadata.is_some_and(|metadata| !metadata.copy) {
        args.push("--no-global-tags".to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no output settings the mkvmerge argument list is unchanged.
    #[test]
    fn mkvmerge_args_without_settings_reproduce_the_previous_layout() {
        let args = mkvmerge_args(
            "out.mkv",
            &["a.mkv".to_owned(), "b.mkv".to_owned()],
            Some("in.mkv"),
            Some(Ratio::new(24_i64, 1)),
            None,
            &[],
            None,
        );
        assert_eq!(args, vec![
            "-o",
            "out.mkv",
            "--no-video",
            "in.mkv",
            "--default-duration",
            "0:24/1fps",
            "[",
            "a.mkv",
            "b.mkv",
            "]",
        ]);
    }
    /// Track selection, per-track options, and video options land in the right
    /// places relative to the source file.
    #[test]
    fn mkvmerge_args_apply_track_selection_and_video_options() {
        let config: MkvmergeConfig = serde_json::from_str(
            r#"{"tracks":{"0":{"type":"video","color_primaries":9},"1":{"type":"audio","copy":false},"2":{"type":"audio","language":"jpn"}}}"#,
        )
        .expect("config should deserialize");
        let args = mkvmerge_args(
            "out.mkv",
            &["a.mkv".to_owned()],
            Some("in.mkv"),
            None,
            Some(&config),
            &[],
            None,
        );
        assert!(args.windows(2).any(|pair| pair == ["--audio-tracks", "!1"]));
        assert!(args.windows(2).any(|pair| pair == ["--language", "2:jpn"]));
        assert!(args.windows(2).any(|pair| pair == ["--color-primaries", "0:9"]));
        let input_position = args.iter().position(|arg| arg == "in.mkv").expect("input path");
        let primaries_position =
            args.iter().position(|arg| arg == "--color-primaries").expect("color primaries");
        assert!(
            primaries_position > input_position,
            "video options apply to the scene group after the input source"
        );
    }
}
