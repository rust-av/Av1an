use super::*;

impl SceneConcatenator {
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn ffmpeg(
        scenes_directory: &Path,
        output: &Path,
        scene_paths: &[PathBuf],
        input: Option<&Path>,
        total_frames: usize,
        framerate: Ratio<i64>,
        config: Option<&FfmpegConfig>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let scratch_directory = scenes_directory.join("Scene Concatenator");
        let concat_file_path = scratch_directory.join("concat.txt");
        let concat_file = {
            let mut contents = String::with_capacity(24 * scene_paths.len());

            for scene_path in scene_paths {
                let fixed_path = scene_path
                    .display()
                    .to_string()
                    .replace('\\', r"\\")
                    .replace(' ', r"\ ")
                    .replace('\'', r"\'");
                contents.push_str("file ");
                contents.push_str(&fixed_path);
                contents.push('\n');
            }

            contents
        };
        File::create(&concat_file_path)?.write_all(concat_file.as_bytes())?;

        let mut cmd = Command::new("ffmpeg");

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.args(ffmpeg_args(&concat_file_path, output, input, config));

        let mut child = cmd.spawn().with_context(|| "Failed to concatenate with FFmpeg")?;
        let stdout = child.stdout.take().expect("FFmpeg should have STDOUT");
        let stderr = child.stderr.take().expect("FFmpeg should have STDERR");

        let stdout_output = Arc::new(Mutex::new(String::new()));
        let stdout_clone = Arc::clone(&stdout_output);
        let stdout_thread = thread::spawn(move || -> Result<()> {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::with_capacity(256);
            loop {
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                            stdout_clone.lock().expect("mutex should acquire lock").push_str(line);
                        }
                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });

        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::with_capacity(256);
        let mut stderr_output = String::new();

        loop {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                child.kill()?;
                child.wait()?;
                return Ok(());
            }

            match reader.read_until(b'\r', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                        stderr_output.push_str(line);
                        if let Some(percentage) =
                            Self::parse_ffmpeg_progress(line, total_frames, framerate)
                        {
                            Self::send_progress(progress_tx, percentage);
                        }
                    }
                    buf.clear();
                },
                Err(e) => return Err(e.into()),
            }
        }

        let status = child.wait().with_context(|| "Failed to wait for FFmpeg")?;
        stdout_thread
            .join()
            .map_err(|_| anyhow::anyhow!("FFmpeg STDOUT thread panicked"))??;
        if !status.success() {
            let error = SceneConcatenatorError::FfmpegFailed {
                status,
                stdout: stdout_output.lock().expect("mutex should acquire lock").clone(),
                stderr: stderr_output,
            };
            error!("{}", error);
            bail!(error);
        }

        Ok(())
    }

    /// Parses a single ffmpeg progress line (e.g. `frame=23310 fps=4835 ...
    /// time=00:15:46.57 ...`)
    #[inline]
    fn parse_ffmpeg_progress(
        line: &str,
        total_frames: usize,
        framerate: Ratio<i64>,
    ) -> Option<f64> {
        if total_frames == 0 {
            return None;
        }

        let current_frames = Self::parse_ffmpeg_frames(line).or_else(|| {
            Self::parse_ffmpeg_time(line).map(|seconds| Self::time_to_frames(seconds, framerate))
        })?;

        Some(current_frames as f64 / total_frames as f64 * 100.0)
    }

    #[inline]
    fn parse_ffmpeg_frames(line: &str) -> Option<u64> {
        let frame = line.split_whitespace().find(|token| token.starts_with("frame="))?;
        frame.strip_prefix("frame=")?.parse().ok()
    }

    #[inline]
    fn parse_ffmpeg_time(line: &str) -> Option<f64> {
        let time = line.split_whitespace().find(|token| token.starts_with("time="))?;
        let time = time.strip_prefix("time=")?;

        // ffmpeg prints time as HH:MM:SS.ms or N.NN
        let seconds = match time.split(':').collect::<Vec<_>>().as_slice() {
            [hours, minutes, seconds] => {
                hours.parse::<f64>().ok()?.mul_add(3600.0, 0.0)
                    + minutes.parse::<f64>().ok()?.mul_add(60.0, 0.0)
                    + seconds.parse::<f64>().ok()?
            },
            [seconds] => seconds.parse::<f64>().ok()?,
            _ => return None,
        };

        Some(seconds)
    }

    #[inline]
    fn time_to_frames(seconds: f64, framerate: Ratio<i64>) -> u64 {
        // framerate = numer/denom frames per second
        (seconds * *framerate.numer() as f64 / *framerate.denom() as f64) as u64
    }
}

/// Builds the FFmpeg argument list for concatenation.
///
/// Without output settings this is the original video-only command. With
/// settings, the original input and any extra inputs are added as `-i`
/// sources, their tracks are mapped according to the configuration, and
/// per-track codecs, filters, and parameters are applied.
fn ffmpeg_args(
    concat_file_path: &Path,
    output: &Path,
    input: Option<&Path>,
    config: Option<&FfmpegConfig>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "info".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        concat_file_path.into(),
    ];

    let Some(config) = config else {
        args.push("-map".into());
        args.push("0".into());
        args.push("-c".into());
        args.push("copy".into());
        args.push(output.into());
        return args;
    };

    // Input 0 is the concatenated video. Input 1 is the original input (when
    // available) and the extra inputs follow it.
    let mut sources: Vec<(&Path, Option<&FfmpegTrackMap>)> = Vec::new();
    if let Some(input) = input {
        sources.push((input, config.tracks.as_ref()));
    }
    for extra in &config.extra_inputs {
        sources.push((extra.path.as_path(), extra.tracks.as_ref()));
    }
    for (path, _) in &sources {
        args.push("-i".into());
        args.push((*path).into());
    }

    // The encoded video track is the only video output; a `Video` entry targets
    // it, and dropping it yields an audio-only file.
    let video_track = config
        .tracks
        .as_ref()
        .and_then(|tracks| tracks.values().find(|track| matches!(track, FfmpegTrack::Video(_))));
    let video_copy = video_track.is_none_or(FfmpegTrack::copy);
    if video_copy {
        args.push("-map".into());
        args.push("0".into());
    }
    args.push("-c".into());
    args.push("copy".into());
    // Only address the video stream when it is actually mapped; a dropped
    // video leaves no `v:0` output for FFmpeg to match.
    if video_copy && let Some(FfmpegTrack::Video(options)) = video_track {
        push_stream_options(&mut args, "v", 0, options);
    }

    let mut audio_output = 0usize;
    let mut subtitle_output = 0usize;
    for (position, (_, tracks)) in sources.iter().enumerate() {
        let input_index = position + 1;
        match tracks {
            None => {
                // Copy every audio, subtitle, and attachment/data stream
                // from this input.
                args.push("-map".into());
                args.push(format!("{input_index}:a?").into());
                args.push("-map".into());
                args.push(format!("{input_index}:s?").into());
                args.push("-map".into());
                args.push(format!("{input_index}:t?").into());
                args.push("-map".into());
                args.push(format!("{input_index}:d?").into());
            },
            Some(tracks) => {
                args.push("-map".into());
                args.push(format!("{input_index}:d?").into());
                let mut ids = tracks.keys().copied().collect::<Vec<_>>();
                ids.sort_unstable();
                for id in ids {
                    let Some(track) = tracks.get(&id) else {
                        continue;
                    };
                    if !track.copy() || matches!(track, FfmpegTrack::Video(_)) {
                        continue;
                    }
                    if let Some(attachment) = track.attachment() {
                        // A bare entry copies the source's attachment stream.
                        // A `path` entry is attached at the output level below,
                        // so it is not mapped from this input.
                        if attachment.path.is_none() {
                            args.push("-map".into());
                            args.push(format!("{input_index}:{id}").into());
                        }
                        continue;
                    }
                    // Remaining tracks are audio or subtitle.
                    match track {
                        FfmpegTrack::Audio(options) => {
                            let index = audio_output;
                            audio_output += 1;
                            args.push("-map".into());
                            args.push(format!("{input_index}:{id}").into());
                            push_stream_options(&mut args, "a", index, options);
                        },
                        FfmpegTrack::Subtitle(options) => {
                            let index = subtitle_output;
                            subtitle_output += 1;
                            args.push("-map".into());
                            args.push(format!("{input_index}:{id}").into());
                            push_stream_options(&mut args, "s", index, options);
                        },
                        FfmpegTrack::Video(_) | FfmpegTrack::Attachment(_) => unreachable!(),
                    }
                }
            },
        }
    }

    // A `path` attachment is a new file attached at the output level via
    // FFmpeg's `-attach`; the `s:t:N` index numbers output attachment
    // streams. Bare (copy) attachments are mapped from their source above.
    let mut attachment_output = 0usize;
    for tracks in std::iter::once(&config.tracks)
        .chain(config.extra_inputs.iter().map(|extra| &extra.tracks))
        .filter_map(Option::as_ref)
    {
        let mut ids = tracks.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        for id in ids {
            let Some(attachment) = tracks.get(&id).and_then(FfmpegTrack::attachment) else {
                continue;
            };
            let Some(path) = &attachment.path else {
                continue;
            };
            args.push("-attach".into());
            args.push(path.into());
            if let Some(mime) = &attachment.attachment_mime_type {
                args.push(format!("-metadata:s:t:{attachment_output}").into());
                args.push(format!("mimetype={mime}").into());
            }
            if let Some(name) = &attachment.attachment_name {
                args.push(format!("-metadata:s:t:{attachment_output}").into());
                args.push(format!("filename={name}").into());
            }
            if let Some(description) = &attachment.attachment_description {
                args.push(format!("-metadata:s:t:{attachment_output}").into());
                args.push(format!("comment={description}").into());
            }
            attachment_output += 1;
        }
    }
    if let Some(chapters) = &config.chapters
        && !chapters.copy
    {
        args.push("-map_chapters".into());
        args.push("-1".into());
    }
    if let Some(metadata) = &config.metadata {
        if !metadata.copy {
            args.push("-map_metadata".into());
            args.push("-1".into());
        }
        if let Some(title) = &metadata.title {
            args.push("-metadata".into());
            args.push(format!("title={title}").into());
        }
        for (key, value) in &metadata.tags {
            args.push("-metadata".into());
            args.push(format!("{key}={value}").into());
        }
    }

    args.push(output.into());
    args
}

fn push_stream_options(
    args: &mut Vec<OsString>,
    stream_type: &str,
    output_index: usize,
    options: &FfmpegTrackOptions,
) {
    if let Some(codec) = &options.codec {
        args.push(format!("-c:{stream_type}:{output_index}").into());
        args.push(codec.into());
    }
    if let Some(filter) = &options.filter {
        args.push(format!("-filter:{stream_type}:{output_index}").into());
        args.push(filter.into());
    }
    for (key, value) in &options.parameters {
        args.push(key.into());
        if !value.is_empty() {
            args.push(value.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }
    /// Without output settings FFmpeg stays video-only.
    #[test]
    fn ffmpeg_args_without_settings_are_video_only() {
        let args = ffmpeg_args(Path::new("concat.txt"), Path::new("out.mkv"), None, None);
        assert_eq!(strings(args), vec![
            "-y",
            "-hide_banner",
            "-loglevel",
            "info",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            "concat.txt",
            "-map",
            "0",
            "-c",
            "copy",
            "out.mkv",
        ]);
    }
    /// A dropped video leaves no `v:0` output stream, so no video stream
    /// options may be emitted.
    #[test]
    fn ffmpeg_args_do_not_address_a_dropped_video() {
        let config: FfmpegConfig = serde_json::from_str(
            r#"{"tracks":{"0":{"type":"video","copy":false,"codec":"libx264"}}}"#,
        )
        .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            None,
            Some(&config),
        ));
        assert!(
            !args.iter().any(|arg| arg.starts_with("-c:v")),
            "no video codec when the video is dropped"
        );
        assert!(
            !args.iter().any(|arg| arg == "-map"),
            "the video is not mapped"
        );
    }
    /// A filtered audio track is mapped from the original input and re-encoded.
    #[test]
    fn ffmpeg_args_add_a_filtered_audio_track() {
        let config: FfmpegConfig = serde_json::from_str(
            r#"{"tracks":{"1":{"type":"audio","codec":"aac","filter":"volume=0.5"}}}"#,
        )
        .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            Some(Path::new("in.mkv")),
            Some(&config),
        ));
        assert!(args.windows(2).any(|pair| pair == ["-i", "in.mkv"]));
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:1"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:a:0", "aac"]));
        assert!(args.windows(2).any(|pair| pair == ["-filter:a:0", "volume=0.5"]));
        assert_eq!(args.last().map(String::as_str), Some("out.mkv"));
    }

    /// A new attachment is written with `-attach` and its metadata.
    #[test]
    fn ffmpeg_args_attach_a_new_file() {
        let config: FfmpegConfig = serde_json::from_str(
            r#"{"tracks":{"0":{"type":"attachment","path":"font.ttf","attachment_name":"My Font","attachment_description":"A font","attachment_mime_type":"font/ttf"}}}"#,
        )
        .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            None,
            Some(&config),
        ));
        assert!(args.windows(2).any(|pair| pair == ["-attach", "font.ttf"]));
        assert!(args.windows(2).any(|pair| pair == ["-metadata:s:t:0", "mimetype=font/ttf"]));
        assert!(args.windows(2).any(|pair| pair == ["-metadata:s:t:0", "filename=My Font"]));
        assert!(args.windows(2).any(|pair| pair == ["-metadata:s:t:0", "comment=A font"]));
    }

    /// An override map cannot name data streams, so they are mapped anyway
    /// rather than dropped by the more specific mapping.
    #[test]
    fn ffmpeg_args_with_an_override_map_still_maps_data_streams() {
        let config: FfmpegConfig = serde_json::from_str(
            r#"{"tracks":{"1":{"type":"audio"},"2":{"type":"audio","copy":false}}}"#,
        )
        .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            Some(Path::new("in.mkv")),
            Some(&config),
        ));
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:1"]));
        assert!(
            !args.iter().any(|arg| arg == "1:2"),
            "the dropped stream is not mapped"
        );
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:d?"]));
    }

    /// Dropping chapters keeps the source's chapters out of the output.
    #[test]
    fn ffmpeg_args_drop_chapters() {
        let config: FfmpegConfig = serde_json::from_str(r#"{"chapters":{"copy":false}}"#)
            .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            Some(Path::new("in.mkv")),
            Some(&config),
        ));
        assert!(args.windows(2).any(|pair| pair == ["-map_chapters", "-1"]));

        let copying: FfmpegConfig = serde_json::from_str(r#"{"chapters":{"copy":true}}"#)
            .expect("config should deserialize");
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            Some(Path::new("in.mkv")),
            Some(&copying),
        ));
        assert!(!args.iter().any(|arg| arg == "-map_chapters"));
    }

    /// Copying every stream also carries attachment (`t`) and data (`d`)
    /// streams, which the audio/subtitle maps alone would drop.
    #[test]
    fn ffmpeg_args_copy_all_maps_attachment_and_data_streams() {
        let config = FfmpegConfig::default();
        let args = strings(ffmpeg_args(
            Path::new("concat.txt"),
            Path::new("out.mkv"),
            Some(Path::new("in.mkv")),
            Some(&config),
        ));
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:t?"]));
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:d?"]));
    }
}
