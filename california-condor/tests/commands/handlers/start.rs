#[cfg(test)]
mod handler_configuration {
    use std::assert_matches;

    use california_condor::{
        commands::handlers::start::start_handler, test_helpers::get_test_video,
        utils::hash_path::hash_path,
    };

    #[test]
    fn start_disable_color_data_turns_appending_off() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let (config, _) = start_handler(
            Some(&config_path),
            None,
            Some(&test_video.path),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("start_handler should succeed");
        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    #[test]
    fn start_missing_main_input_returns_error_without_creating_configuration() {
        let invalid_input = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&invalid_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let result = start_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&invalid_input),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        );
        assert_matches!(result, Err(_), "start_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }
}
