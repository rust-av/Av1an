use super::*;

#[test]
fn avm_one_pass_argv_omits_pass_and_statistics_options() {
    // The pinned AVM CLI already defaults to one pass; do not copy the AOM fix.
    let output = Path::new("job space 映像/scene0.ivf");
    assert_eq!(
        EncoderBase::AVM.pass_parameters((1, 1), "unused").expect("params"),
        HashMap::new()
    );
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    encoder.set_append_color_data(false);
    assert_eq!(
        encoder
            .parameters_with_output((1, 1), output, &crate::core::encoder::tests::clip_info())
            .expect("argv"),
        ["-", "-o", output.to_str().expect("utf8")]
    );
}
