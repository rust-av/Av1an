#![cfg(test)]
use super::*;

mod need_api_and_vsscript {
    use std::fmt::Debug;
    use std::sync::mpsc::channel;

    use super::*;
    use function::Function;
    use prelude::*;
    use video_info::{Framerate, Resolution};

    fn props_test(frame: &Frame, fps_num: i64) {
        let props = frame.props();
        assert_eq!(props.key_count(), 2);
        assert_eq!(props.key(0), "_DurationDen");
        assert_eq!(props.key(1), "_DurationNum");

        assert_eq!(props.value_count(props.key(0)), Ok(1));
        assert_eq!(props.get_int(props.key(0)), Ok(fps_num));
        assert_eq!(props.value_count(props.key(1)), Ok(1));
        assert_eq!(props.get_int(props.key(1)), Ok(1));
    }

    fn env_video_var_test(env: &vsscript::Environment) {
        let mut map = OwnedMap::new(API::get().unwrap());
        assert!(env.get_variable("video", &mut map).is_ok());
        assert_eq!(map.key_count(), 1);
        assert_eq!(map.key(0), "video");
        let node = map.get_video_node("video");
        assert!(node.is_ok());
    }

    fn green_frame_test(frame: &Frame) {
        let format = frame.format();
        assert_eq!(format.name(), "RGB24");
        assert_eq!(format.plane_count(), 3);

        for plane in 0..format.plane_count() {
            let resolution = frame.resolution(plane);
            assert_eq!(
                resolution,
                Resolution {
                    width: 1920,
                    height: 1080,
                }
            );

            let color = if plane == 1 { [255; 1920] } else { [0; 1920] };

            for row in 0..resolution.height {
                let data_row = frame.data_row(plane, row);
                assert_eq!(data_row, &color[..]);
                let data_row = frame.plane_row::<u8>(plane, row);
                assert_eq!(data_row, &color[..]);
            }
        }
    }

    fn green_test(env: &vsscript::Environment) {
        let (node, alpha_node) = {
            let output = env.get_output(0);
            assert!(output.is_ok());
            output.unwrap()
        };
        assert!(alpha_node.is_none());

        let info = node.info();
        assert_eq!(info.format.name(), "RGB24");

        assert_eq!(
            info.framerate,
            Property::Constant(Framerate {
                numerator: 60,
                denominator: 1,
            })
        );
        assert_eq!(
            info.resolution,
            Property::Constant(Resolution {
                width: 1920,
                height: 1080,
            })
        );

        assert_eq!(info.num_frames, 100);

        let frame = node.get_frame(0).unwrap();
        green_frame_test(&frame);
        props_test(&frame, 60);
        env_video_var_test(env);
    }

    #[test]
    fn green() {
        let env =
            vsscript::Environment::from_file("test-vpy/green.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();
        green_test(&env);
    }

    #[test]
    fn green_from_string() {
        let env =
            vsscript::Environment::from_script(include_str!("../test-vpy/green.vpy")).unwrap();
        green_test(&env);
    }

    #[test]
    fn variable() {
        let env =
            vsscript::Environment::from_file("test-vpy/variable.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let (node, alpha_node) = {
            let output = env.get_output(0);
            assert!(output.is_ok());
            output.unwrap()
        };
        assert!(alpha_node.is_none());

        let info = node.info();

        assert_eq!(info.format.name(), "Undefined");
        assert_eq!(info.framerate, Property::Variable);
        assert_eq!(info.resolution, Property::Variable);

        assert_eq!(info.num_frames, 200);

        // Test the first frame.
        let frame = node.get_frame(0).unwrap();
        let format = frame.format();
        assert_eq!(format.name(), "RGB24");
        assert_eq!(format.plane_count(), 3);

        for plane in 0..format.plane_count() {
            let resolution = frame.resolution(plane);
            assert_eq!(
                resolution,
                Resolution {
                    width: 1920,
                    height: 1080,
                }
            );

            let color = if plane == 1 { [255; 1920] } else { [0; 1920] };

            for row in 0..resolution.height {
                let data_row = frame.data_row(plane, row);
                assert_eq!(data_row, &color[..]);
                let data_row = frame.plane_row::<u8>(plane, row);
                assert_eq!(data_row, &color[..]);
            }
        }

        props_test(&frame, 60);

        // Test the first frame of the next format.
        let frame = node.get_frame(100).unwrap();
        let format = frame.format();
        assert_eq!(format.name(), "Gray8");
        assert_eq!(format.plane_count(), 1);

        let plane = 0;
        let resolution = frame.resolution(plane);
        assert_eq!(
            resolution,
            Resolution {
                width: 1280,
                height: 720,
            }
        );

        let color = [127; 1280];

        for row in 0..resolution.height {
            let data_row = frame.data_row(plane, row);
            assert_eq!(data_row, &color[..]);
            let data_row = frame.plane_row::<u8>(plane, row);
            assert_eq!(data_row, &color[..]);
        }

        props_test(&frame, 30);
        env_video_var_test(&env);
    }

    #[test]
    fn alpha() {
        let env =
            vsscript::Environment::from_file("test-vpy/alpha.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let (_, alpha_node) = {
            let output = env.get_output(0);
            assert!(output.is_ok());
            output.unwrap()
        };

        assert!(alpha_node.is_some());
        let alpha_node = alpha_node.unwrap();

        let info = alpha_node.info();
        assert_eq!(info.format.name(), "Gray8");

        assert_eq!(
            info.framerate,
            Property::Constant(Framerate {
                numerator: 60,
                denominator: 1,
            })
        );
        assert_eq!(
            info.resolution,
            Property::Constant(Resolution {
                width: 1920,
                height: 1080,
            })
        );

        assert_eq!(info.num_frames, 100);

        let frame = alpha_node.get_frame(0).unwrap();
        let format = frame.format();
        assert_eq!(format.name(), "Gray8");
        assert_eq!(format.plane_count(), 1);

        let resolution = frame.resolution(0);
        assert_eq!(
            resolution,
            Resolution {
                width: 1920,
                height: 1080,
            }
        );

        for row in 0..resolution.height {
            let data_row = frame.data_row(0, row);
            assert_eq!(data_row, &[128; 1920][..]);
            let data_row = frame.plane_row::<u8>(0, row);
            assert_eq!(data_row, &[128; 1920][..]);
        }
    }

    fn verify_pixel_format<T: Component + Debug + Copy + PartialEq>(
        env: &Environment,
        index: i32,
        bits_per_sample: u8,
        color: [T; 3],
    ) {
        let node = env.get_output(index).unwrap().0;
        let frame = node.get_frame(0).unwrap();
        let format = frame.format();

        assert_eq!(format.bits_per_sample(), bits_per_sample);
        let bytes_per_sample = bits_per_sample.div_ceil(8).next_power_of_two();
        assert_eq!(format.bytes_per_sample(), bytes_per_sample);

        for plane_num in 0..3 {
            // Compare the entire row at once for speed.
            let row_gt = vec![color[plane_num]; frame.width(plane_num)];

            for y in 0..frame.height(plane_num) {
                let row = frame.plane_row(plane_num, y);
                assert_eq!(row.len(), frame.width(plane_num));
                assert_eq!(&row_gt[..], row);
            }

            if let Ok(data) = frame.plane(plane_num) {
                assert_eq!(data.len(), frame.height(plane_num) * frame.width(plane_num));

                for y in 0..frame.height(plane_num) {
                    assert_eq!(
                        &row_gt[..],
                        &data[y * frame.width(plane_num)..(y + 1) * frame.width(plane_num)]
                    );
                }
            }
        }
    }

    #[test]
    fn pixel_formats() {
        let env = vsscript::Environment::from_file(
            "test-vpy/pixel-formats.vpy",
            vsscript::EvalFlags::Nothing,
        )
        .unwrap();

        verify_pixel_format(&env, 0, 10, [789u16, 123u16, 456u16]);
        verify_pixel_format(&env, 1, 32, [5f32, 42f32, 0.25f32]);
        verify_pixel_format(&env, 2, 32, [0.125f32, 10f32, 0.5f32]);

        #[cfg(feature = "f16-pixel-type")]
        verify_pixel_format(
            &env,
            3,
            16,
            [
                half::f16::from_f32(0.0625f32),
                half::f16::from_f32(5f32),
                half::f16::from_f32(0.25f32),
            ],
        );
    }

    #[test]
    #[should_panic]
    fn invalid_component_type() {
        let env = vsscript::Environment::from_file(
            "test-vpy/pixel-formats.vpy",
            vsscript::EvalFlags::Nothing,
        )
        .unwrap();

        let node = env.get_output(0).unwrap().0;

        let frame = node.get_frame(0).unwrap();
        let _ = frame.plane_row::<u8>(0, 0); // Should be u16.
    }

    #[test]
    fn gradient() {
        let env =
            vsscript::Environment::from_file("test-vpy/gradient.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let node = env.get_output(0).unwrap().0;

        let frame = node.get_frame(0).unwrap();
        for plane in 0..3 {
            for row in 0..16 {
                let mut gt = [0u8; 16];
                for (col, item) in gt.iter_mut().enumerate() {
                    *item = match plane {
                        0 => row as u8 * 16,
                        1 => col as u8 * 16,
                        2 => 0,
                        _ => unreachable!(),
                    };
                }

                let data = frame.data_row(plane, row);
                assert_eq!(data, &gt[..]);
                let data = frame.plane_row::<u8>(plane, row);
                assert_eq!(data, &gt[..]);
            }
        }
    }

    #[test]
    fn clear_output() {
        let env =
            vsscript::Environment::from_script(include_str!("../test-vpy/green.vpy")).unwrap();
        // In v4, clear_output is a no-op that always succeeds
        assert!(env.clear_output(1).is_ok());
        assert!(env.clear_output(0).is_ok());
        assert!(env.clear_output(0).is_ok());
    }

    #[test]
    fn iterators() {
        let env =
            vsscript::Environment::from_script(include_str!("../test-vpy/green.vpy")).unwrap();

        let (node, alpha_node) = env.get_output(0).unwrap();

        assert!(alpha_node.is_none());

        let frame = node.get_frame(0).unwrap();
        let props = frame.props();

        assert_eq!(props.keys().size_hint(), (2, Some(2)));
        // assert_eq!(props.iter().size_hint(), (2, Some(2)));
    }

    #[test]
    fn vsscript_variables() {
        let env =
            vsscript::Environment::from_script(include_str!("../test-vpy/green.vpy")).unwrap();

        let mut map = OwnedMap::new(API::get().unwrap());
        assert!(env.get_variable("video", &mut map).is_ok());
        // In v4, clear_variable is a no-op that always succeeds
        assert!(env.clear_variable("video").is_ok());
        assert!(env.clear_variable("video").is_ok());

        assert!(env.set_variables(&map).is_ok());
        assert!(env.get_variable("video", &mut map).is_ok());
    }

    #[test]
    fn get_frame_async() {
        let env =
            vsscript::Environment::from_file("test-vpy/green.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let (node, alpha_node) = {
            let output = env.get_output(0);
            assert!(output.is_ok());
            output.unwrap()
        };

        assert!(alpha_node.is_none());

        let mut rxs = Vec::new();

        for i in 0..10 {
            let (tx, rx) = channel();
            rxs.push(rx);

            node.get_frame_async(i, move |frame, n, node| {
                assert!(frame.is_ok());
                let frame = frame.unwrap();

                assert_eq!(n, i);
                assert_eq!(
                    node.info().framerate,
                    Property::Constant(Framerate {
                        numerator: 60,
                        denominator: 1,
                    })
                );

                green_frame_test(&frame);
                props_test(&frame, 60);

                assert_eq!(tx.send(()), Ok(()));
            });
        }

        drop(node); // Test dropping prematurely.

        for rx in rxs {
            assert_eq!(rx.recv(), Ok(()));
        }
    }

    #[test]
    fn get_frame_async_error() {
        let env =
            vsscript::Environment::from_file("test-vpy/green.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let (node, alpha_node) = {
            let output = env.get_output(0);
            assert!(output.is_ok());
            output.unwrap()
        };

        assert!(alpha_node.is_none());

        let (tx, rx) = channel();

        // The clip only has 100 frames, so requesting the 101th one produces an error.
        node.get_frame_async(100, move |frame, n, node| {
            assert!(frame.is_err());
            assert_eq!(n, 100);
            assert_eq!(
                node.info().framerate,
                Property::Constant(Framerate {
                    numerator: 60,
                    denominator: 1,
                })
            );

            assert_eq!(tx.send(()), Ok(()));
        });

        assert_eq!(rx.recv(), Ok(()));
    }

    #[test]
    fn core() {
        let env =
            vsscript::Environment::from_file("test-vpy/green.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let core = env.get_core();
        assert!(core.is_ok());
        let core = core.unwrap();

        let yuv420p8 = core.get_format(PresetFormat::YUV420P8.into());
        assert!(yuv420p8.is_some());
        let yuv420p8 = yuv420p8.unwrap();

        assert_eq!(yuv420p8.id(), PresetFormat::YUV420P8.into());
        assert_eq!(yuv420p8.name(), "YUV420P8");
        assert_eq!(yuv420p8.plane_count(), 3);
        assert_eq!(yuv420p8.color_family(), ColorFamily::YUV);
        assert_eq!(yuv420p8.sample_type(), SampleType::Integer);
        assert_eq!(yuv420p8.bits_per_sample(), 8);
        assert_eq!(yuv420p8.bytes_per_sample(), 1);
        assert_eq!(yuv420p8.sub_sampling_w(), 1);
        assert_eq!(yuv420p8.sub_sampling_h(), 1);

        let yuv422p8 = core.get_format(PresetFormat::YUV422P8.into()).unwrap();
        assert_eq!(yuv422p8.sub_sampling_w(), 1);
        assert_eq!(yuv422p8.sub_sampling_h(), 0);

        assert_eq!(core.set_max_cache_size(1337), 1337);
        assert_eq!(core.set_thread_count(3), 3);
        assert_eq!(core.info().max_framebuffer_size, 1337);
        assert_eq!(core.info().num_threads, 3);
    }

    #[test]
    fn functions() {
        let env =
            vsscript::Environment::from_file("test-vpy/green.vpy", vsscript::EvalFlags::Nothing)
                .unwrap();

        let core = env.get_core().unwrap();
        let api = API::get().unwrap();

        let function = Function::new(api, core, |_api, _core, in_, out| {
            assert_eq!(in_.get_int("hello").unwrap(), 1337);
            out.set_int("there", 42).unwrap();
        });

        let mut in_ = OwnedMap::new(api);
        let mut out = OwnedMap::new(api);
        in_.set_int("hello", 1337).unwrap();

        function.call(&in_, &mut out);

        assert!(out.error().is_none());
        assert_eq!(out.get_int("there").unwrap(), 42);
    }
}

// We need either VSScript or the VapourSynth functions.
mod need_api {
    use super::*;
    use prelude::*;

    #[test]
    fn maps() {
        let mut map = OwnedMap::new(API::get().unwrap());

        assert_eq!(map.key_count(), 0);

        assert_eq!(map.set_int("i", 42), Ok(()));
        assert_eq!(map.get_int("i"), Ok(42));
        assert_eq!(map.append_int("i", 43), Ok(()));
        assert_eq!(map.get_int("i"), Ok(42));
        {
            let iter = map.get_int_iter("i");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(42));
            assert_eq!(iter.next(), Some(43));
            assert_eq!(iter.next(), None);
        }

        assert_eq!(map.set("i", &42), Ok(()));
        assert_eq!(map.get("i"), Ok(42));
        assert_eq!(map.append("i", &43), Ok(()));
        assert_eq!(map.get("i"), Ok(42));
        {
            let iter = map.get_iter::<i64>("i");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(42));
            assert_eq!(iter.next(), Some(43));
            assert_eq!(iter.next(), None);
        }

        assert_eq!(map.get_int_array("i"), Ok(&[42, 43][..]));

        assert_eq!(map.set_int_array("ia", &[10, 20, 30]), Ok(()));
        assert_eq!(map.get_int_array("ia"), Ok(&[10, 20, 30][..]));

        assert_eq!(map.set_float("f", 42f64), Ok(()));
        assert_eq!(map.get_float("f"), Ok(42f64));
        assert_eq!(map.append_float("f", 43f64), Ok(()));
        assert_eq!(map.get_float("f"), Ok(42f64));
        {
            let iter = map.get_float_iter("f");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(42f64));
            assert_eq!(iter.next(), Some(43f64));
            assert_eq!(iter.next(), None);
        }

        assert_eq!(map.set("f", &42f64), Ok(()));
        assert_eq!(map.get("f"), Ok(42f64));
        assert_eq!(map.append("f", &43f64), Ok(()));
        assert_eq!(map.get("f"), Ok(42f64));
        {
            let iter = map.get_iter::<f64>("f");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(42f64));
            assert_eq!(iter.next(), Some(43f64));
            assert_eq!(iter.next(), None);
        }

        assert_eq!(map.get_float_array("f"), Ok(&[42f64, 43f64][..]));

        assert_eq!(map.set_float_array("fa", &[10f64, 20f64, 30f64]), Ok(()));
        assert_eq!(map.get_float_array("fa"), Ok(&[10f64, 20f64, 30f64][..]));

        assert_eq!(map.set_data("d", &[1, 2, 3]), Ok(()));
        assert_eq!(map.get_data("d"), Ok(&[1, 2, 3][..]));
        assert_eq!(map.append_data("d", &[4, 5, 6]), Ok(()));
        assert_eq!(map.get_data("d"), Ok(&[1, 2, 3][..]));
        {
            let iter = map.get_data_iter("d");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(&[1, 2, 3][..]));
            assert_eq!(iter.next(), Some(&[4, 5, 6][..]));
            assert_eq!(iter.next(), None);
        }

        assert_eq!(map.set("d", &&[1, 2, 3][..]), Ok(()));
        assert_eq!(map.get("d"), Ok(&[1, 2, 3][..]));
        assert_eq!(map.append("d", &&[4, 5, 6][..]), Ok(()));
        assert_eq!(map.get("d"), Ok(&[1, 2, 3][..]));
        {
            let iter = map.get_iter::<&[u8]>("d");
            assert!(iter.is_ok());
            let mut iter = iter.unwrap();
            assert_eq!(iter.next(), Some(&[1, 2, 3][..]));
            assert_eq!(iter.next(), Some(&[4, 5, 6][..]));
            assert_eq!(iter.next(), None);
        }

        // TODO: node, frame and function method tests when we can make them.

        assert_eq!(map.delete_key("i"), Ok(()));
        assert_eq!(map.delete_key("i"), Err(map::Error::KeyNotFound));

        assert_eq!(map.error(), None);
        assert_eq!(map.set_error("hello there"), Ok(()));
        assert_eq!(
            map.error().as_ref().map(|x| x.as_ref()),
            Some("hello there")
        );
    }

    #[test]
    fn create_core() {
        let api = API::get().unwrap();
        let core = api.create_core(4);
        assert_eq!(core.info().num_threads, 4);

        let yuv420p8 = core.get_format(PresetFormat::YUV420P8.into());
        assert!(yuv420p8.is_some());
        let yuv420p8 = yuv420p8.unwrap();

        assert_eq!(yuv420p8.id(), PresetFormat::YUV420P8.into());
        assert_eq!(yuv420p8.name(), "YUV420P8");
        assert_eq!(yuv420p8.plane_count(), 3);
        assert_eq!(yuv420p8.color_family(), ColorFamily::YUV);
        assert_eq!(yuv420p8.sample_type(), SampleType::Integer);
        assert_eq!(yuv420p8.bits_per_sample(), 8);
        assert_eq!(yuv420p8.bytes_per_sample(), 1);
        assert_eq!(yuv420p8.sub_sampling_w(), 1);
        assert_eq!(yuv420p8.sub_sampling_h(), 1);

        let yuv422p8 = core.get_format(PresetFormat::YUV422P8.into()).unwrap();
        assert_eq!(yuv422p8.sub_sampling_w(), 1);
        assert_eq!(yuv422p8.sub_sampling_h(), 0);

        assert_eq!(core.set_max_cache_size(1337), 1337);
        assert_eq!(core.set_thread_count(3), 3);
        assert_eq!(core.info().max_framebuffer_size, 1337);
        assert_eq!(core.info().num_threads, 3);
    }
}
