#[cfg(test)]
mod tests {
    use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;

    fn rejects(input: &str, context: &[&str]) {
        let error = input.parse::<VapourSynthFilter>().expect_err(input).to_string();
        for part in context {
            assert!(
                error.contains(part),
                "{input:?}: expected {part:?} in {error:?}"
            );
        }
    }

    macro_rules! invalid_filters {
    ($name:ident, $(($input:expr, [$($context:expr),*])),+ $(,)?) => {
        #[test]
        fn $name() {
            $(rejects($input, &[$($context),*]);)+
        }
    };
}

    invalid_filters!(
        crop_errors,
        ("crop:top=abc", ["crop", "top", "abc"]),
        ("crop:top=-1", ["crop", "top", "-1"]),
        ("crop:left=4294967296", ["crop", "left", "4294967296"]),
        (
            "crop:bottom=18446744073709551616",
            ["crop", "bottom", "18446744073709551616"]
        ),
    );
    invalid_filters!(
        resize_errors,
        ("resize:width=abc", ["resize", "width", "abc"]),
        ("resize:width=0", ["resize", "width", "0"]),
        (
            "resize:height=4294967296",
            ["resize", "height", "4294967296"]
        ),
        ("resize:scaler=雪", ["resize", "scaler", "雪"]),
        ("resize:format=unknown", ["resize", "format", "unknown"]),
    );
    invalid_filters!(
        trim_errors,
        ("trim:start=abc", ["trim", "start", "abc"]),
        ("trim:end=-1", ["trim", "end", "-1"]),
        ("trim:end=4294967296", ["trim", "end", "4294967296"]),
        ("trim:start=10;end=5", ["trim", "end", "5", "10"]),
    );
    invalid_filters!(
        rescale_errors,
        (
            "rescale:width=1280;height=720;doubler=ArtCNN",
            ["rescale", "kernel", "missing"]
        ),
        (
            "rescale:kernel=Bicubic;height=720;doubler=ArtCNN",
            ["rescale", "width", "missing"]
        ),
        (
            "rescale:kernel=Bicubic;width=1280;doubler=ArtCNN",
            ["rescale", "height", "missing"]
        ),
        (
            "rescale:kernel=Bicubic;width=1280;height=720",
            ["rescale", "doubler", "missing"]
        ),
        (
            "rescale:kernel=bad;width=1280;height=720;doubler=ArtCNN",
            ["rescale", "kernel", "bad"]
        ),
        (
            "rescale:kernel=Bicubic;width=0;height=720;doubler=ArtCNN",
            ["rescale", "width", "0"]
        ),
        (
            "rescale:kernel=Bicubic;width=1280;height=720;doubler=bad",
            ["rescale", "doubler", "bad"]
        ),
    );
    invalid_filters!(
        wnnm_errors,
        ("wnnm:sigma=1,abc,0", ["wnnm", "sigma", "abc"]),
        ("wnnm:sigma=", ["wnnm", "sigma", "\"\""]),
        ("wnnm:sigma=1,,0", ["wnnm", "sigma", "1,,0"]),
        ("wnnm:residual=maybe", ["wnnm", "residual", "maybe"]),
        (
            "wnnm:adaptive_aggregation=2",
            ["wnnm", "adaptive_aggregation", "2"]
        ),
        (
            "wnnm:block_size=4294967296",
            ["wnnm", "block_size", "4294967296"]
        ),
    );
    invalid_filters!(
        bilateral_errors,
        ("bilateral:sigma_s=abc", ["bilateral", "sigma_s", "abc"]),
        (
            "bilateral:planes=true,maybe",
            ["bilateral", "planes", "maybe"]
        ),
        ("bilateral:planes=", ["bilateral", "planes", "\"\""]),
        ("bilateral:algorithm=0,3", ["bilateral", "algorithm", "0,3"]),
        ("bilateral:pbficnum=1", ["bilateral", "pbficnum", "1"]),
        ("bilateral:pbficnum=257", ["bilateral", "pbficnum", "257"]),
        (
            "bilateral:algorithm=4294967296",
            ["bilateral", "algorithm", "4294967296"]
        ),
    );
    invalid_filters!(
        degrain_errors,
        ("degrain:blksize=8,abc", ["degrain", "blksize", "abc"]),
        ("degrain:chroma=maybe", ["degrain", "chroma", "maybe"]),
        ("degrain:thsad=1,", ["degrain", "thsad", "1,"]),
        ("degrain:pel=4294967296", ["degrain", "pel", "4294967296"]),
        ("degrain:planes=-1", ["degrain", "planes", "-1"]),
    );
    invalid_filters!(
        zoom_degrain_errors,
        ("zoom_degrain:hpad=abc", ["zoom_degrain", "hpad", "abc"]),
        (
            "zoom_degrain:trymany=maybe",
            ["zoom_degrain", "trymany", "maybe"]
        ),
        (
            "zoom_degrain:plane=2147483648",
            ["zoom_degrain", "plane", "2147483648"]
        ),
        ("zoom_degrain:plane=-2", ["zoom_degrain", "plane", "-2"]),
        (
            "zoom_degrain:plane=-1",
            ["zoom_degrain", "plane", "-1", "0..=4"]
        ),
        (
            "zoom_degrain:plane=5",
            ["zoom_degrain", "plane", "5", "0..=4"]
        ),
        (
            "zoom_degrain:recalc_overlap=4294967296",
            ["zoom_degrain", "recalc_overlap", "4294967296"]
        ),
    );
    invalid_filters!(
        fgs_errors,
        ("fgs:", ["fgs", "iso", "missing"]),
        ("fgs:iso=abc", ["fgs", "iso", "abc"]),
        ("fgs:iso=4294967296", ["fgs", "iso", "4294967296"]),
        ("fgs:iso=0;cy=128", ["fgs", "cy", "128"]),
        ("fgs:iso=0;ccb=1,abc", ["fgs", "ccb", "abc"]),
        ("fgs:iso=0;ccr=", ["fgs", "ccr", "\"\""]),
        ("fgs:iso=0;dynamic_seed=yes", ["fgs", "dynamic_seed", "yes"]),
    );
    invalid_filters!(
        output_format_errors,
        ("output-format:width=abc", ["output-format", "width", "abc"]),
        (
            "output_format:bit_depth=9",
            ["output_format", "bit_depth", "9"]
        ),
        (
            "output-format:chroma=bad",
            ["output-format", "chroma", "bad"]
        ),
        ("output-format:height=0", ["output-format", "height", "0"]),
        (
            "output-format:width=2147483648",
            ["output-format", "width", "2147483648"]
        ),
        (
            "output-format:width=1:ignored",
            ["output-format", "width", "1:ignored"]
        ),
    );

    #[test]
    fn grammar_rejects_ambiguous_arguments() {
        rejects("crop", &["crop", ":"]);
        rejects("", &["filter"]);
        rejects(":top=1", &["filter", "name"]);
        rejects("Crop:top=1", &["Crop"]);
        for name in [
            "crop",
            "resize",
            "trim",
            "rescale",
            "wnnm",
            "bilateral",
            "degrain",
            "zoom_degrain",
            "fgs",
            "output-format",
            "output_format",
        ] {
            rejects(&format!("{name}:typo=1"), &[name, "typo"]);
        }
        for (input, field) in [
            ("crop:top", "top"),
            ("crop:=1", "key"),
            ("crop:top=", "top"),
            ("crop:top=1;TOP=2", "top"),
            ("output-format:width=1;WIDTH=2", "width"),
        ] {
            rejects(input, &[field]);
        }
    }

    #[test]
    fn all_float_fields_require_finite_values() {
        for (filter, field) in [
            ("wnnm", "sigma"),
            ("bilateral", "sigma_s"),
            ("bilateral", "sigma_r"),
            ("degrain", "limit"),
            ("degrain", "thscd2"),
        ] {
            for value in ["NaN", "inf", "-inf"] {
                rejects(
                    &format!("{filter}:{field}={value}"),
                    &[filter, field, value],
                );
            }
        }
        rejects("degrain:limit=1,NaN", &["degrain", "limit", "NaN"]);
    }

    #[test]
    fn fgs_coefficient_lengths_are_checked_before_execution() {
        for (field, length) in [("cy", 25), ("ccb", 26), ("ccr", 26)] {
            let value = vec!["0"; length].join(",");
            rejects(
                &format!("fgs:iso=0;{field}={value}"),
                &["fgs", field, &value],
            );
        }
    }

    #[test]
    fn zoom_plane_range_matches_native_selections() {
        for plane in 0..=4 {
            let input = format!("zoom_degrain:plane={plane}");
            let VapourSynthFilter::ZoomDegrain { plane: actual, .. } =
                input.parse().expect("native plane selection")
            else {
                panic!("zoom_degrain")
            };
            assert_eq!(actual, Some(plane));
        }
        let VapourSynthFilter::ZoomDegrain { plane, .. } =
            "zoom_degrain:".parse().expect("native default")
        else {
            panic!("zoom_degrain")
        };
        assert_eq!(plane, None);
    }

    #[test]
    fn rescale_does_not_inherit_unrelated_u32_narrowing() {
        if usize::BITS > 32 {
            let filter = "rescale:kernel=Mitchell;width=4294967296;height=720;doubler=ArtCNN"
                .parse::<VapourSynthFilter>()
                .expect("rescale emits f64 sizes, not u32 plugin arguments");
            let VapourSynthFilter::Rescale { width, .. } = filter else {
                panic!("rescale")
            };
            assert_eq!(width as u64, 4294967296);
        }
    }

    #[test]
    fn valid_defaults_sentinels_and_roundtrips() {
        for input in [
            "crop:",
            "crop: TOP = 0;bottom=1;",
            "resize:",
            "trim:start=0;end=0;",
            "trim:",
            "rescale:kernel=Mitchell;width=1280;height=720;doubler=ArtCNN;",
            "wnnm:",
            "wnnm:sigma=3, 0,-1;radius=0;residual=1;adaptive_aggregation=0;",
            "bilateral:",
            "bilateral:planes=true, 0,1,false;algorithm=0,1,2;pbficnum=2,256;",
            "degrain:",
            "degrain:overlap=0,0;chroma=1;recalculate=false;limit=0,1;thscd2=0;",
            "zoom_degrain:",
            "zoom_degrain:levels=0;overlap=0;plane=4;limit=0;trymany=true;recalc_smooth=0;",
            "fgs:iso=0;chroma_iso=0;cy=-128,127;dynamic_seed=0;",
            "output-format:bit_depth=10;",
            "output_format:CHROMA=444;width=16;",
        ] {
            let filter = input.parse::<VapourSynthFilter>().expect(input);
            assert_eq!(
                filter.to_string().parse::<VapourSynthFilter>().expect(input),
                filter,
                "{input}"
            );
        }
        for prefix in ["雪:C:\\clips=raw", "", "  雪:MiXeD=raw  "] {
            let input = format!("degrain:prefix={prefix};");
            let VapourSynthFilter::Degrain { prefix: actual, .. } =
                input.parse().expect("Unicode prefix")
            else {
                panic!("degrain")
            };
            assert_eq!(actual.as_deref(), Some(prefix));
        }
        assert!(matches!(
            "output-format:bit_depth=10".parse::<VapourSynthFilter>().expect("deferred"),
            VapourSynthFilter::OutputFormat {
                bit_depth: Some(10),
                chroma: None,
                width: None,
                height: None
            }
        ));
    }
}
