use super::*;
use av_format::rational::Ratio;

pub(super) fn job(
    root: &Path,
    output: &Path,
    count: usize,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    let mut job = concat_job(root, output);
    job.input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=6,format=vs.YUV420P8).set_output()".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
    }).expect("finite native input");
    job.sequence_config.parallel_encoder.scenes_directory = root.into();
    job.scenes = (0..count)
        .map(|index| Scene {
            start_frame: index * 2,
            end_frame: index * 2 + 2,
            sub_scenes: None,
            encoder: Default::default(),
            sequence_data: Default::default(),
        })
        .collect();
    job
}

fn native(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("required native fixture tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

pub(super) fn decoded(path: &Path) -> Vec<u8> {
    native(
        Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-",
        ]),
    )
}

pub(super) fn methods() -> [ConcatMethod; 3] {
    [
        ConcatMethod::Ivf,
        ConcatMethod::FFmpeg(FfmpegConfig::default()),
        ConcatMethod::MKVMerge(Default::default()),
    ]
}

pub(super) struct NativeScenes {
    directory: tempfile::TempDir,
    pub(super) job: Condor<DefaultSequenceData, DefaultSequenceConfig>,
    pub(super) paths: Vec<PathBuf>,
    pub(super) frames: Vec<Vec<u8>>,
    originals: Vec<Vec<u8>>,
}

impl NativeScenes {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().expect("native fixture");
        let root = directory.path();
        let mut job = job(root, &root.join("final.ivf"), 3);
        let paths = (0..3).map(|i| root.join(format!("{i:05}.ivf"))).collect::<Vec<_>>();
        for (path, color) in paths.iter().zip(["white", "black", "red"]) {
            native(
                Command::new("ffmpeg")
                    .args(["-v", "error", "-f", "lavfi", "-i"])
                    .arg(format!("color={color}:size=32x16:rate=24"))
                    .args([
                        "-frames:v",
                        "2",
                        "-c:v",
                        "libaom-av1",
                        "-cpu-used",
                        "8",
                        "-crf",
                        "0",
                        "-g",
                        "1",
                    ])
                    .arg(path),
            );
        }
        let originals =
            paths.iter().map(|p| std::fs::read(p).expect("native scene bytes")).collect();
        let frames = paths.iter().map(|p| decoded(p)).collect::<Vec<_>>();
        assert!(frames.iter().all(|f| f.len() == 32 * 16 * 3 / 2 * 2));
        assert!(
            frames.windows(2).all(|pair| pair[0] != pair[1]),
            "visibly distinct native scenes"
        );
        let scratch = root.join("Scene Concatenator");
        std::fs::create_dir_all(&scratch).expect("caller scratch");
        std::fs::write(scratch.join("sentinel"), b"caller scratch sentinel").expect("sentinel");
        std::fs::write(scratch.join("source.mkv"), b"caller fixed scratch sentinel")
            .expect("sentinel");
        let protected = root.join("source-config.vpy");
        std::fs::write(&protected, b"declared source config sentinel").expect("source config");
        job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(protected),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        });
        Self {
            directory,
            job,
            paths,
            frames,
            originals,
        }
    }

    pub(super) fn root(&self) -> &Path {
        self.directory.path()
    }

    pub(super) fn requested(&self) -> [PathBuf; 4] {
        [
            self.paths[2].clone(),
            self.paths[0].clone(),
            self.paths[1].clone(),
            self.paths[2].clone(),
        ]
    }

    pub(super) fn concat_direct(&self, index: usize, method: &ConcatMethod) -> PathBuf {
        let output = self.root().join(format!("direct-{index}.{}", method.extension()));
        let paths = self.requested();
        let (tx, _rx) = sync::mpsc::channel();
        let token = Cancellation::default();
        match method {
            ConcatMethod::Ivf => SceneConcatenator::ivf_cancel(&output, &paths, &tx, &token),
            ConcatMethod::FFmpeg(_) => SceneConcatenator::ffmpeg_cancel(
                self.root(),
                &output,
                &paths,
                None,
                8,
                Ratio::new(24, 1),
                None,
                &tx,
                &token,
            ),
            ConcatMethod::MKVMerge(_) => SceneConcatenator::mkvmerge_cancel(
                self.root(),
                &output,
                &paths,
                None,
                Ratio::new(24, 1),
                None,
                &tx,
                &token,
            ),
        }
        .expect("complete direct concat");
        output
    }

    pub(super) fn prepare_final(
        &mut self,
        index: usize,
        method: ConcatMethod,
    ) -> (PathBuf, Option<Vec<u8>>) {
        let old = (index != 0).then(|| {
            std::fs::read(self.concat_direct(index, &method)).expect("distinct valid old mux")
        });
        let final_path = self.root().join(format!("final-{index}.{}", method.extension()));
        self.job.output.path = self.root().join(format!("unused-fallback-{index}.mkv"));
        self.job.sequence_config.scene_concatenator.method = method;
        self.job.sequence_config.scene_concatenator.output = Some(final_path.clone());
        if let Some(bytes) = &old {
            std::fs::write(&final_path, bytes).expect("old valid output with explicit consent");
        }
        (final_path, old)
    }

    pub(super) fn assert_unchanged(self) {
        for (path, bytes) in self.paths.iter().zip(self.originals) {
            assert_eq!(std::fs::read(path).expect("input intact"), bytes);
        }
        let root = self.directory.path();
        let scratch = root.join("Scene Concatenator");
        assert_eq!(
            std::fs::read(scratch.join("sentinel")).expect("caller scratch intact"),
            b"caller scratch sentinel"
        );
        assert_eq!(
            std::fs::read(scratch.join("source.mkv")).expect("fixed scratch intact"),
            b"caller fixed scratch sentinel"
        );
        assert_eq!(
            std::fs::read(root.join("source-config.vpy")).expect("declared source intact"),
            b"declared source config sentinel"
        );
        assert_clean(root);
        self.directory.close().expect("native fixture cleanup");
    }
}

pub(super) fn assert_clean(root: &Path) {
    assert!(
        !std::fs::read_dir(root).expect("staging cleanup").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".condor-output-"))
    );
}
