//! Private IVF splitter contracts; concat-only integrations live with SceneConcatenator.
use super::*;

#[path = "../../../../../tests/core/sequence/scene_concatenator/fixtures/ivf_support.rs"]
mod fixture_support;
use fixture_support::{assert_output, decoded, fixture, native, records, set_timestamps};

fn tasks(dir: &Path, counts: &[usize]) -> Vec<Task> {
    counts
        .iter()
        .enumerate()
        .map(|(index, &count)| Task {
            original_index: index,
            index,
            frame_indices: (0..count).collect(),
            sub_scenes: None,
            encoder: Encoder::default_from_base(&EncoderBase::AOM, false),
            output: dir.join(format!("part{index}.ivf")),
        })
        .collect()
}

mod internal_integration {
    use super::*;

    #[test]
    fn private_ivf_split_preserves_native_payloads_and_normalizes_part_pts() {
        let dir = tempfile::tempdir().expect("whole-stream fixture directory");
        let whole = dir.path().join("whole.ivf");
        native(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=24000/1001",
                "-frames:v",
                "9",
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "8",
                "-crf",
                "0",
                "-g",
                "1",
            ],
            Some(&whole),
        );
        let mut bytes = fs::read(&whole).expect("read whole IVF");
        set_timestamps(&mut bytes, &[900, 901, 902, 903, 904, 905, 906, 907, 908]);
        fs::write(&whole, &bytes).expect("set nonzero whole origin");
        let original_pixels = decoded(&whole);
        assert_eq!(original_pixels.len(), 64 * 64 * 3 / 2 * 9);
        assert!(original_pixels.as_chunks::<6144>().0.windows(2).all(|p| p[0] != p[1]));
        let tasks = tasks(dir.path(), &[2, 3, 4]);
        let split = split_ivf(&whole, &tasks, "ivf", true).expect("split native whole IVF");
        let paths = tasks.iter().map(|t| t.output.clone()).collect::<Vec<_>>();
        let mut payloads = Vec::new();
        for (index, (path, expected)) in
            paths.iter().zip([vec![0, 1], vec![0, 1, 2], vec![0, 1, 2, 3]]).enumerate()
        {
            assert_output(path, expected.len() as u32, &bytes[16..24], &expected);
            let part = fs::read(path).expect("read split part");
            assert_eq!(split[index], (part.len() as u64, expected.len()));
            payloads.extend(records(&part).into_iter().map(|r| r.1));
        }
        assert_eq!(
            payloads,
            records(&bytes).into_iter().map(|r| r.1).collect::<Vec<_>>()
        );
    }

    #[test]
    fn private_ivf_split_refuses_invalid_pts_and_normalizes_each_part_origin() {
        let dir = tempfile::tempdir().expect("boundary fixture directory");
        let a = dir.path().join("a.ivf");
        let split_tasks = tasks(dir.path(), &[2]);
        for timestamps in [vec![9, 8], vec![9, 9]] {
            fixture(&a, &timestamps);
            let error = split_ivf(&a, &split_tasks, "ivf", false)
                .expect_err("decreasing/duplicate split timestamp");
            assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
            assert!(!split_tasks[0].output.exists());
            assert!(!split_tasks[0].output.with_extension("temp.ivf").exists());
        }
        for offset in [16, 20] {
            fixture(&a, &[0, 1]);
            let mut bytes = fs::read(&a).expect("read zero rate/scale fixture");
            bytes[offset..offset + 4].fill(0);
            fs::write(&a, bytes).expect("write zero rate/scale fixture");
            assert!(split_ivf(&a, &split_tasks, "ivf", false).is_err());
        }
        let boundary_dir = dir.path().join("boundary");
        fs::create_dir_all(&boundary_dir).expect("split boundary directory");
        let boundary_tasks = tasks(&boundary_dir, &[2, 1]);
        fixture(&a, &[u64::MAX - 5, u64::MAX - 3, u64::MAX]);
        split_ivf(&a, &boundary_tasks, "ivf", false).expect("split near u64 max");
        assert_eq!(
            records(&fs::read(&boundary_tasks[0].output).expect("read first boundary part"))
                .iter()
                .map(|r| r.0)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert_eq!(
            records(&fs::read(&boundary_tasks[1].output).expect("read second boundary part"))
                .iter()
                .map(|r| r.0)
                .collect::<Vec<_>>(),
            [0]
        );
    }
}
