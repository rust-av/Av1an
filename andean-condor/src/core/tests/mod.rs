use super::*;

#[test]
fn the_temp_save_path_keeps_one_dot_before_the_extension() {
    for (save_file, expected) in [
        ("condor.json", "condor.temp.json"),
        ("condor", "condor.temp"),
        ("a.b.json", "a.b.temp.json"),
    ] {
        assert_eq!(
            temp_save_path(Path::new(save_file)),
            PathBuf::from(expected),
            "{save_file}"
        );
    }
}
