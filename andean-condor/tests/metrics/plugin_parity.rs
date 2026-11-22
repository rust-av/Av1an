//! Real GPU/plugin acceptance. ABI marker libraries cannot establish accuracy.
#[cfg(test)]
mod tests {
    use andean_condor::{
        metrics::{probe::OutputIndexing, vship},
        models::sequence::target_quality::types::QualityMetric,
        vapoursynth::{
            VapourSynthError, get_core,
            plugins::vship::cvvdp::{CVVDP, DisplayModel},
        },
    };

    // Fresh three-frame, nonuniform SDR fixtures with deliberately different errors.
    // The pinned plugin forces BT.709 transfer/primaries and limited input range,
    // ignoring frame props. At this short height it also forces a BT.601 matrix:
    // neutral chroma makes that matrix difference irrelevant for this fixture.
    // Native input must use BT.709 transfer too (not piecewise sRGB). Both paths
    // use 24 fps, standard_fhd, no resize, GPU 0 and the same temporal flag.
    // _Range=0 and deprecated _ColorRange=1 both mean limited, not the same code.
    const INPUT: &str = "import vapoursynth as vs
core = vs.core
def tile(y):
    return core.std.BlankClip(width=128, height=128, length=1, fpsnum=24, fpsden=1, format=vs.YUV444P8, color=[y,128,128])
def picture(n, shift):
    ys = [40+n*8, 180-n*9, 90+n*13, 140-n*7]
    t = [tile(y+shift) for y in ys]
    return core.std.StackVertical([core.std.StackHorizontal(t[:2]),core.std.StackHorizontal(t[2:])])
r = core.std.Splice([picture(n,0) for n in range(3)]).std.SetFrameProps(_Matrix=1, _Transfer=1, _Primaries=1, _Range=0, _ColorRange=1)
d = core.std.Splice([picture(n,s) for n,s in enumerate([1,30,7])]).std.SetFrameProps(_Matrix=1, _Transfer=1, _Primaries=1, _Range=0, _ColorRange=1)
";

    fn decoder(output: &str) -> anyhow::Result<av_decoders::Decoder> {
        Ok(av_decoders::Decoder::from_script(
            &format!("{INPUT}{output}"),
            Default::default(),
        )?)
    }

    #[test]
    fn cvvdp_parity_fixture_decodes_expected_bt709_yuv444_samples() {
        // Fixture setup only: no metric/plugin/GPU is invoked by this check.
        for (output, shifts) in [("r", [0, 0, 0]), ("d", [1, 30, 7])] {
            let mut decoder = decoder(&format!("{output}.set_output()")).expect("real fixture");
            let details = decoder
                .get_vapoursynth_impl()
                .expect("VS decoder")
                .get_extended_video_details()
                .expect("decoded metadata");
            assert_eq!(
                details.color_transfer_characteristic,
                Some(av_decoders::TransferCharacteristic::BT709)
            );
            assert_eq!(
                (details.width, details.height, details.bit_depth),
                (256, 256, 8)
            );
            assert_eq!(details.frame_rate, av_decoders::Rational32::new(24, 1));
            assert_eq!(
                details.chroma_sampling,
                av_decoders::v_frame::chroma::ChromaSubsampling::Yuv444
            );
            let color = av_metrics_vship::colorspace_from_details(&details, None)
                .expect("native input tags");
            assert_eq!(
                color.transfer_function,
                av_metrics_vship::VshipTransferFunction::Bt709
            );
            assert_eq!(color.range, av_metrics_vship::VshipRange::Limited);
            for (n, shift) in shifts.into_iter().enumerate() {
                let frame = decoder.read_video_frame::<u8>().expect("actual decoded pixels");
                let geometry = frame.y_plane.geometry();
                let ys = [40 + n * 8, 180 - n * 9, 90 + n * 13, 140 - n * 7];
                for (tile, (x, y)) in
                    [(0, 0), (128, 0), (0, 128), (128, 128)].into_iter().enumerate()
                {
                    assert_eq!(
                        frame.y_plane.data()[geometry.data_origin() + y * geometry.stride() + x],
                        (ys[tile] + shift) as u8
                    );
                }
                for plane in [frame.u_plane.as_ref(), frame.v_plane.as_ref()] {
                    let plane = plane.expect("YUV444 chroma");
                    let g = plane.geometry();
                    for row in 0..g.height() {
                        let start = g.data_origin() + row * g.stride();
                        assert!(
                            plane.data()[start..start + g.width()]
                                .iter()
                                .all(|&sample| sample == 128),
                            "neutral chroma is required: plugin ignores the short-height BT.709 matrix tag"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn plugin_gateway_rejects_source_gaps_before_plugin_invocation() {
        use andean_condor::vapoursynth::plugins::PluginFunction;

        let mut decoder =
            decoder("r.set_output(0)\nd.set_output(1)").expect("real compacted nodes");
        let vs = decoder.get_vapoursynth_impl().expect("VS fixture");
        let core = get_core(&vs.env).expect("real core");
        let reference = vs.get_output(0, vs.get_node_modifier()).expect("reference");
        let distorted = vs.get_output(1, vs.get_node_modifier()).expect("distorted");
        // No plugin/device is needed: original indices must reject before invoke,
        // even though the spliced nodes themselves have contiguous indices 0..3.
        for flag in [None, Some(false)] {
            let error = CVVDP {
                disable_temporal: flag,
                ..Default::default()
            }
            .invoke(core, &reference, &distorted, [0, 1, 10])
            .expect_err("plugin cannot preserve accumulation while resetting gap history");
            assert!(
                matches!(&error, VapourSynthError::PluginFunctionError { .. }),
                "{error}"
            );
            let message = error.to_string();
            assert!(
                message.contains("history-only")
                    && message.contains("disable_temporal=true")
                    && message.contains("native Vship"),
                "{message}"
            );
        }
        // When absent, supported cases must reach the real missing-plugin error
        // rather than the gap rejection. Never initialize a real GPU in this test.
        if !CVVDP::plugin_is_installed(core) {
            for (flag, indices) in
                [(None, [10, 11, 12]), (Some(false), [0, 1, 2]), (Some(true), [0, 1, 10])]
            {
                let error = CVVDP {
                    disable_temporal: flag,
                    ..Default::default()
                }
                .invoke(core, &reference, &distorted, indices)
                .expect_err("plugin absent");
                assert!(
                    matches!(
                        error,
                        VapourSynthError::PluginNotFound { .. }
                            | VapourSynthError::PluginLoadError { .. }
                    ),
                    "{error}"
                );
            }
        }
    }

    fn native(disabled: bool, selected: &[usize]) -> anyhow::Result<Vec<f64>> {
        let mut reference = decoder("r.set_output()")?;
        let mut distorted = decoder("d.set_output()")?;
        vship::score_probed_frames(
            &mut reference,
            &mut distorted,
            &QualityMetric::CVVDP {
                target_range: (0.0, 10.0),
                resolution: None,
                display_model: Some(DisplayModel::StandardFHD),
                resize_to_display: Some(false),
                disable_temporal: Some(disabled),
                gpu_id: Some(0),
            },
            selected,
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
    }

    fn plugin(disabled: bool) -> anyhow::Result<Vec<f64>> {
        let mut decoder = decoder("r.set_output(0)\nd.set_output(1)")?;
        let vs = decoder.get_vapoursynth_impl().expect("VS fixture");
        let core = get_core(&vs.env)?;
        let reference = vs.get_output(0, vs.get_node_modifier())?;
        let distorted = vs.get_output(1, vs.get_node_modifier())?;
        // Actual production plugin wrapper, not a copied spelling of its arguments.
        let node = CVVDP {
            gpu_id: Some(0),
            model_name: Some(DisplayModel::StandardFHD),
            resize_to_display: Some(false),
            disable_temporal: Some(disabled),
            distmap: Some(false),
            ..Default::default()
        }
        .invoke(core, &reference, &distorted, [0, 1, 2])?;
        (0..3)
            .map(|index| Ok(node.get_frame(index)?.props().get_float("_CVVDP")?))
            .collect()
    }

    #[test]
    fn cvvdp_native_and_plugin_scores_agree_with_temporal_processing_enabled_and_disabled() {
        if !av_metrics_vship::is_available() {
            assert!(
                std::env::var_os("AV_METRICS_REQUIRE_CVVDP_PARITY").is_none(),
                "required temporal parity needs pinned Vship 5.1.2, supported GPU and VS plugin"
            );
            eprintln!("UNEXECUTED temporal numerical parity: no supported Vship/GPU runtime");
            return;
        }
        let input = decoder("r.set_output()").expect("fresh GPU fixture");
        let format = av_metrics_vship::VideoFormat::from_details(input.get_video_details());
        let scorer = av_metrics_vship::VshipScorer::new(
            av_metrics_vship::VshipConfig::default()
                .with_metric(av_metrics_vship::VshipMetric::Cvvdp)
                .with_gpu_id(0)
                .with_fps(24.0),
            format,
            format,
            None,
        )
        .expect("GPU handler");
        assert!(
            matches!(
                scorer.backend(),
                av_metrics_vship::VshipBackend::Hip
                    | av_metrics_vship::VshipBackend::Cuda
                    | av_metrics_vship::VshipBackend::Vulkan
            ),
            "temporal numerical acceptance requires a real GPU backend, not CPU/ABI markers"
        );
        drop(scorer);
        let disabled_plugin = match plugin(true) {
            Ok(scores) => scores,
            Err(error) => {
                assert!(
                    matches!(
                        error.downcast_ref::<VapourSynthError>(),
                        Some(
                            VapourSynthError::PluginNotFound { .. }
                                | VapourSynthError::PluginLoadError { .. }
                                | VapourSynthError::PluginFunctionNotFound { .. }
                        )
                    ),
                    "temporal plugin execution failed (not missing runtime): {error:#}"
                );
                assert!(
                    std::env::var_os("AV_METRICS_REQUIRE_CVVDP_PARITY").is_none(),
                    "required temporal plugin parity unavailable: {error:#}"
                );
                eprintln!("UNEXECUTED temporal numerical parity: plugin unavailable: {error:#}");
                return;
            },
        };
        let disabled = native(true, &[0, 1, 2]).expect("disabled native CVVDP");
        let enabled = native(false, &[0, 1, 2]).expect("enabled native CVVDP");
        let enabled_plugin = plugin(false).expect("enabled plugin CVVDP");
        // Provisional same-backend absolute JOD tolerance; not locally calibrated.
        let close = |a: f64, b: f64| {
            assert!(
                a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-4,
                "temporal parity outside provisional 1e-4 JOD tolerance: {a} vs {b}"
            );
        };
        for index in 0..3 {
            close(disabled[index], disabled_plugin[index]);
            close(
                disabled[index],
                native(true, &[index]).expect("fresh independent scorer")[0],
            );
            close(enabled[index], enabled_plugin[index]);
        }
        assert!(
            enabled.iter().zip(&disabled).skip(1).any(|(a, b)| (a - b).abs() > 1e-6),
            "fixture must distinguish enabled history/accumulation from independent frames"
        );
    }
}
