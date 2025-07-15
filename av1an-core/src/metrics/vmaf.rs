use std::{
    cmp::Ordering,
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{anyhow, Context};
use plotters::prelude::*;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    broker::EncoderCrash,
    ffmpeg,
    ref_smallvec,
    util::printable_base10_digits,
    Input,
    VmafFeature,
};

#[derive(Deserialize, Serialize, Debug)]
struct VmafScore {
    vmaf: f64,
}

#[derive(Deserialize, Serialize, Debug)]
struct Metrics {
    metrics: VmafScore,
}

#[derive(Deserialize, Serialize, Debug)]
struct VmafResult {
    frames: Vec<Metrics>,
}

pub fn plot_vmaf_score_file(scores_file: &Path, plot_path: &Path) -> anyhow::Result<()> {
    let scores = read_vmaf_file(scores_file).with_context(|| "Failed to parse VMAF file")?;

    let mut sorted_scores = scores.clone();
    sorted_scores.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Less));

    let plot_width = 1600 + (printable_base10_digits(scores.len()) * 200);
    let plot_heigth = 600;

    let length = scores.len() as u32;

    let root =
        SVGBackend::new(plot_path.as_os_str(), (plot_width, plot_heigth)).into_drawing_area();

    root.fill(&WHITE)?;

    let perc_1 = percentile_of_sorted(&sorted_scores, 0.01);
    let perc_25 = percentile_of_sorted(&sorted_scores, 0.25);
    let perc_50 = percentile_of_sorted(&sorted_scores, 0.50);
    let perc_75 = percentile_of_sorted(&sorted_scores, 0.75);

    let mut chart = ChartBuilder::on(&root)
        .set_label_area_size(LabelAreaPosition::Bottom, (5).percent())
        .set_label_area_size(LabelAreaPosition::Left, (5).percent())
        .set_label_area_size(LabelAreaPosition::Right, (7).percent())
        .set_label_area_size(LabelAreaPosition::Top, (5).percent())
        .margin((1).percent())
        .build_cartesian_2d(0_u32..length, perc_1.floor()..100.0)?;

    chart.configure_mesh().draw()?;

    // 1%
    chart
        .draw_series(LineSeries::new((0..=length).map(|x| (x, perc_1)), RED))?
        .label(format!("1%: {perc_1}"))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));

    // 25%
    chart
        .draw_series(LineSeries::new((0..=length).map(|x| (x, perc_25)), YELLOW))?
        .label(format!("25%: {perc_25}"))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], YELLOW));

    // 50% (median, except not averaged in the case of an even number of elements)
    chart
        .draw_series(LineSeries::new((0..=length).map(|x| (x, perc_50)), BLACK))?
        .label(format!("50%: {perc_50}"))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLACK));

    // 75%
    chart
        .draw_series(LineSeries::new((0..=length).map(|x| (x, perc_75)), GREEN))?
        .label(format!("75%: {perc_75}"))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));

    // Data
    chart.draw_series(LineSeries::new(
        (0..).zip(scores.iter()).map(|(x, y)| (x, *y)),
        BLUE,
    ))?;

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;

    root.present().expect("Unable to write result plot to file");

    Ok(())
}

pub fn validate_libvmaf() -> anyhow::Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-h");

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let out = cmd.output()?;

    let stdr = String::from_utf8(out.stderr)?;
    if !stdr.contains("--enable-libvmaf") {
        return Err(anyhow!(
            "FFmpeg is not compiled with --enable-libvmaf, but target quality or VMAF plotting \
             was enabled"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn plot(
    encoded: &Path,
    reference: &Input,
    model: Option<impl AsRef<Path>>,
    res: &str,
    scaler: &str,
    sample_rate: usize,
    filter: Option<&str>,
    threads: usize,
    probing_vmaf_features: &[VmafFeature],
) -> Result<(), Box<EncoderCrash>> {
    let json_file = encoded.with_extension("json");
    let plot_file = encoded.with_extension("svg");
    let vspipe_args;

    println!(":: VMAF Run");

    let pipe_cmd: SmallVec<[&OsStr; 8]> = match reference {
        Input::Video {
            ref path, ..
        } => {
            vspipe_args = vec![];
            ref_smallvec!(OsStr, 8, [
                "ffmpeg",
                "-i",
                path,
                "-strict",
                "-1",
                "-f",
                "yuv4mpegpipe",
                "-"
            ])
        },
        Input::VapourSynth {
            ref path,
            vspipe_args: args,
            ..
        } => {
            vspipe_args = args.to_owned();
            ref_smallvec!(OsStr, 8, ["vspipe", "-c", "y4m", path, "-"])
        },
    };

    run_vmaf(
        encoded,
        &pipe_cmd,
        vspipe_args,
        &json_file,
        model,
        res,
        scaler,
        sample_rate,
        filter,
        threads,
        60.0,
        false,
        probing_vmaf_features,
    )?;

    plot_vmaf_score_file(&json_file, &plot_file).unwrap();
    Ok(())
}

pub fn get_vmaf_model_version(features: &[VmafFeature]) -> &'static str {
    let has_uhd = features.contains(&VmafFeature::Uhd);
    let has_neg = features.contains(&VmafFeature::Neg);

    match (has_uhd, has_neg) {
        (true, true) => "vmaf_4k_v0.6.1neg",
        (true, false) => "vmaf_4k_v0.6.1",
        (false, true) => "vmaf_v0.6.1neg",
        (false, false) => "vmaf_v0.6.1",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_vmaf(
    encoded: &Path,
    reference_pipe_cmd: &[impl AsRef<OsStr>],
    vspipe_args: Vec<String>,
    stat_file: impl AsRef<Path>,
    model: Option<impl AsRef<Path>>,
    res: &str,
    scaler: &str,
    sample_rate: usize,
    vmaf_filter: Option<&str>,
    threads: usize,
    framerate: f64,
    disable_motion: bool,
    probing_vmaf_features: &[VmafFeature],
) -> Result<(), Box<EncoderCrash>> {
    let mut filter = if sample_rate > 1 {
        format!(
            "select=not(mod(n\\,{})),setpts={:.4}*PTS,",
            sample_rate,
            1.0 / sample_rate as f64,
        )
    } else {
        String::new()
    };

    if let Some(vmaf_filter) = vmaf_filter {
        filter.reserve(1 + vmaf_filter.len());
        filter.push_str(vmaf_filter);
        filter.push(',');
    }

    let vmaf = if let Some(model) = model {
        let model_path = if model.as_ref().as_os_str().to_string_lossy().ends_with(".json") {
            format!(
                "version={}{}",
                get_vmaf_model_version(probing_vmaf_features),
                if disable_motion {
                    "\\:motion.motion_force_zero=true"
                } else {
                    ""
                }
            )
        } else {
            format!("path={}", ffmpeg::escape_path_in_filter(&model))
        };
        format!(
            "[distorted][ref]libvmaf=log_fmt='json':eof_action=endall:log_path={}:model='{}':\
             n_threads={}",
            ffmpeg::escape_path_in_filter(stat_file),
            model_path,
            threads
        )
    } else {
        format!(
            "[distorted][ref]libvmaf=log_fmt='json':eof_action=endall:log_path={}{}:n_threads={}",
            ffmpeg::escape_path_in_filter(stat_file),
            if disable_motion {
                format!(
                    ":model='version={}\\:motion.motion_force_zero=true'",
                    get_vmaf_model_version(probing_vmaf_features)
                )
            } else {
                String::new()
            },
            threads
        )
    };

    let mut source_pipe = if let [cmd, args @ ..] = reference_pipe_cmd {
        let mut source_pipe = Command::new(cmd);
        // Append vspipe python arguments to the environment if there are any
        for arg in vspipe_args {
            source_pipe.args(["-a", &arg]);
        }
        source_pipe.args(args);
        source_pipe.stdout(Stdio::piped());
        source_pipe.stderr(Stdio::null());
        source_pipe.spawn().unwrap()
    } else {
        unreachable!()
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-loglevel",
        "error",
        "-hide_banner",
        "-nostdin",
        "-y",
        "-thread_queue_size",
        "1024",
        "-r",
        &framerate.to_string(),
        "-i",
    ]);
    cmd.arg(encoded);
    cmd.args(["-r", &framerate.to_string(), "-i", "-", "-filter_complex"]);

    let distorted = format!(
        "[0:v]scale={}:flags={}:force_original_aspect_ratio=decrease,setpts=PTS-STARTPTS,\
         setsar=1[distorted];",
        &res, &scaler
    );
    let reference = format!(
        "[1:v]{}scale={}:flags={}:force_original_aspect_ratio=decrease,setpts=PTS-STARTPTS,\
         setsar=1[ref];",
        filter, &res, &scaler
    );

    cmd.arg(format!("{distorted}{reference}{vmaf}"));
    cmd.args(["-f", "null", "-"]);
    cmd.stdin(source_pipe.stdout.take().unwrap());
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::null());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        return Err(Box::new(EncoderCrash {
            exit_status:        output.status,
            source_pipe_stderr: String::new().into(),
            ffmpeg_pipe_stderr: None,
            stderr:             output.stderr.into(),
            stdout:             String::new().into(),
        }));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_vmaf_weighted(
    encoded: &Path,
    reference_pipe_cmd: &[impl AsRef<OsStr>],
    vspipe_args: Vec<String>,
    model: Option<impl AsRef<Path>>,
    _res: &str,
    _scaler: &str,
    sample_rate: usize,
    vmaf_filter: Option<&str>,
    threads: usize,
    framerate: f64,
    disable_motion: bool,
    probing_vmaf_features: &[VmafFeature],
) -> anyhow::Result<Vec<f64>> {
    let temp_dir = encoded.parent().unwrap();
    let vmaf_y_path = temp_dir.join(format!(
        "vmaf_y_{}.json",
        encoded.file_stem().unwrap().to_str().unwrap()
    ));
    let vmaf_u_path = temp_dir.join(format!(
        "vmaf_u_{}.json",
        encoded.file_stem().unwrap().to_str().unwrap()
    ));
    let vmaf_v_path = temp_dir.join(format!(
        "vmaf_v_{}.json",
        encoded.file_stem().unwrap().to_str().unwrap()
    ));

    let mut filter = if sample_rate > 1 {
        format!(
            "select=not(mod(n\\,{})),setpts={:.4}*PTS,",
            sample_rate,
            1.0 / sample_rate as f64,
        )
    } else {
        String::new()
    };

    if let Some(vmaf_filter) = vmaf_filter {
        filter.reserve(1 + vmaf_filter.len());
        filter.push_str(vmaf_filter);
        filter.push(',');
    }

    let model_str = if let Some(model) = model {
        if model.as_ref().as_os_str().to_string_lossy().ends_with(".json") {
            format!(
                "version={}{}",
                get_vmaf_model_version(probing_vmaf_features),
                if disable_motion {
                    "\\:motion.motion_force_zero=true"
                } else {
                    ""
                }
            )
        } else {
            format!(
                "path={}{}",
                ffmpeg::escape_path_in_filter(&model),
                if disable_motion {
                    "\\:motion.motion_force_zero=true"
                } else {
                    ""
                }
            )
        }
    } else {
        format!(
            "version={}{}",
            get_vmaf_model_version(probing_vmaf_features),
            if disable_motion {
                "\\:motion.motion_force_zero=true"
            } else {
                ""
            }
        )
    };

    let mut source_pipe = if let [cmd, args @ ..] = reference_pipe_cmd {
        let mut source_pipe = Command::new(cmd);
        for arg in vspipe_args {
            source_pipe.args(["-a", &arg]);
        }
        source_pipe.args(args);
        source_pipe.stdout(Stdio::piped());
        source_pipe.stderr(Stdio::null());
        source_pipe.spawn().unwrap()
    } else {
        unreachable!()
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-loglevel",
        "error",
        "-hide_banner",
        "-nostdin",
        "-y",
        "-thread_queue_size",
        "1024",
        "-r",
        &framerate.to_string(),
        "-i",
    ]);
    cmd.arg(encoded);
    cmd.args(["-r", &framerate.to_string(), "-i", "-", "-filter_complex"]);

    let filter_complex = format!(
        "[1:v]format=yuv420p[ref];[0:v]format=yuv420p[dis];\
         [dis]extractplanes=y+u+v[dis_y][dis_u][dis_v];\
         [ref]extractplanes=y+u+v[ref_y][ref_u][ref_v];[dis_y][ref_y]libvmaf=log_path={}:\
         log_fmt=json:n_threads={}:n_subsample=1:model='{}':eof_action=endall[vmaf_y_out];\
         [dis_u][ref_u]libvmaf=log_path={}:log_fmt=json:n_threads={}:n_subsample=1:model='{}':\
         eof_action=endall[vmaf_u_out];[dis_v][ref_v]libvmaf=log_path={}:log_fmt=json:\
         n_threads={}:n_subsample=1:model='{}':eof_action=endall[vmaf_v_out]",
        ffmpeg::escape_path_in_filter(&vmaf_y_path),
        threads,
        model_str,
        ffmpeg::escape_path_in_filter(&vmaf_u_path),
        threads,
        model_str,
        ffmpeg::escape_path_in_filter(&vmaf_v_path),
        threads,
        model_str
    );

    cmd.arg(filter_complex);
    cmd.args(["-map", "[vmaf_y_out]", "-map", "[vmaf_u_out]", "-map", "[vmaf_v_out]"]);
    cmd.args(["-an", "-sn", "-dn", "-f", "null", "-"]);
    cmd.stdin(source_pipe.stdout.take().unwrap());
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::null());

    let output = cmd.output().unwrap();

    let _ = source_pipe.wait();

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "FFmpeg command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    if !vmaf_y_path.exists() || !vmaf_u_path.exists() || !vmaf_v_path.exists() {
        return Err(anyhow::anyhow!("Y, U, V VMAF files were not created"));
    }

    let y_scores = read_vmaf_file(&vmaf_y_path).context("Failed to read VMAF Y scores")?;
    let u_scores = read_vmaf_file(&vmaf_u_path).context("Failed to read VMAF U scores")?;
    let v_scores = read_vmaf_file(&vmaf_v_path).context("Failed to read VMAF V scores")?;

    let weighted_scores = y_scores
        .iter()
        .zip(u_scores.iter())
        .zip(v_scores.iter())
        .map(|((y, u), v)| (4.0 * y + u + v) / 6.0)
        .collect();

    Ok(weighted_scores)
}

pub fn read_vmaf_file(file: impl AsRef<Path>) -> Result<Vec<f64>, serde_json::Error> {
    let json_str = std::fs::read_to_string(file).unwrap();
    let vmaf_results = serde_json::from_str::<VmafResult>(&json_str)?;
    let v = vmaf_results.frames.into_iter().map(|metric| metric.metrics.vmaf).collect();

    Ok(v)
}

pub fn percentile_of_sorted(scores: &[f64], percentile: f64) -> f64 {
    assert!(!scores.is_empty());

    let k = ((scores.len() - 1) as f64 * percentile) as usize;

    scores[k]
}
