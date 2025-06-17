use core::f64;
use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{broker::EncoderCrash, ffmpeg};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, IntoStaticStr,
)]
pub enum XPSNRSubMetric {
    #[strum(serialize = "xpsnr-minimum")]
    Minimum,
    #[strum(serialize = "xpsnr-weighted")]
    Weighted,
}

pub fn validate_libxpsnr() -> anyhow::Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-filters");

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let out = cmd.output()?;

    let stdr = String::from_utf8(out.stdout)?;
    if !stdr.contains("xpsnr") {
        return Err(anyhow!(
            "FFmpeg is not compiled with XPSNR or is outdated, but target quality or XPSNR \
             plotting was enabled"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_xpsnr(
    encoded: &Path,
    reference_pipe_cmd: &[impl AsRef<OsStr>],
    vspipe_args: Vec<String>,
    stat_file: impl AsRef<Path>,
    res: &str,
    scaler: &str,
    sample_rate: usize,
    framerate: f64,
) -> Result<(), Box<EncoderCrash>> {
    let filter = if sample_rate > 1 {
        format!(
            "select=not(mod(n\\,{})),setpts={:.4}*PTS,",
            sample_rate,
            1.0 / sample_rate as f64,
        )
    } else {
        String::new()
    };

    let xpsnr = format!(
        "[distorted][ref]xpsnr=stats_file={}:eof_action=endall",
        ffmpeg::escape_path_in_filter(stat_file)
    );

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
        "-y",
        "-thread_queue_size",
        "1024",
        "-hide_banner",
        "-r",
        &framerate.to_string(),
        "-i",
    ]);
    cmd.arg(encoded);
    cmd.args(["-r", &framerate.to_string(), "-i", "-", "-filter_complex"]);

    let distorted = format!(
        "[0:v]scale={}:flags={}:force_original_aspect_ratio=decrease,setsar=1[distorted];",
        &res, &scaler
    );
    let reference = format!(
        "[1:v]{filter}scale={}:flags={}:force_original_aspect_ratio=decrease,setsar=1[ref];",
        &res, &scaler
    );

    cmd.arg(format!("{distorted}{reference}{xpsnr}"));
    cmd.args(["-f", "null", "-"]);
    cmd.stdin(source_pipe.stdout.take().unwrap());
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::null());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        println!(
            "FFmpeg exited with status {}",
            String::from_utf8(output.stdout.clone()).unwrap()
        );
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

pub fn read_xpsnr_file(file: impl AsRef<Path>, submetric: XPSNRSubMetric) -> (f64, Vec<f64>) {
    let log_str = std::fs::read_to_string(file).unwrap();
    let frame_regex = regex::Regex::new(
        r".*XPSNR y: *([0-9\.]+|inf) *XPSNR u: *([0-9\.]+|inf) *XPSNR v: *([0-9\.]+|inf)",
    )
    .unwrap();

    let final_line = log_str
        .lines()
        .find(|line| line.contains("XPSNR average"))
        .expect("No average XPSNR line found");

    let final_regex = regex::Regex::new(
        r"XPSNR average, \d+ frames  y: *([0-9\.]+|inf)  u: *([0-9\.]+|inf)  v: *([0-9\.]+|inf)  \(minimum: *([0-9\.]+|inf)\)",
    )
    .unwrap();

    let parse_float_or_inf = |s: &str| {
        if s == "inf" {
            f64::INFINITY
        } else {
            s.parse::<f64>().unwrap_or(f64::INFINITY)
        }
    };
    let final_captures = final_regex.captures(final_line).unwrap();
    let final_yuv = (
        parse_float_or_inf(&final_captures[1]),
        parse_float_or_inf(&final_captures[2]),
        parse_float_or_inf(&final_captures[3]),
    );

    let mut frame_values = Vec::new();
    for line in log_str.lines() {
        if let Some(captures) = frame_regex.captures(line) {
            if submetric == XPSNRSubMetric::Minimum {
                let min_psnr = captures
                    .iter()
                    .skip(1)
                    .map(|value| parse_float_or_inf(value.unwrap().as_str()))
                    .fold(f64::INFINITY, f64::min);
                frame_values.push(min_psnr);
            } else {
                let parsed_values: Vec<f64> = captures
                    .iter()
                    .skip(1)
                    .map(|value| parse_float_or_inf(value.unwrap().as_str()))
                    .collect();

                let weighted =
                    ((4.0 * parsed_values[0]) + parsed_values[1] + parsed_values[2]) / 6.0;
                frame_values.push(weighted);
            }
        }
    }

    match submetric {
        XPSNRSubMetric::Minimum => (final_yuv.0.min(final_yuv.1).min(final_yuv.2), frame_values),
        XPSNRSubMetric::Weighted => {
            let weighted = weight_xpsnr(final_yuv.0, final_yuv.1, final_yuv.2);

            (weighted, frame_values)
        },
    }
}

pub fn weight_xpsnr(y: f64, u: f64, v: f64) -> f64 {
    -10.0
        * f64::log10(
            ((4.0 * f64::powf(10.0, -y / 10.0))
                + f64::powf(10.0, -u / 10.0)
                + f64::powf(10.0, -v / 10.0))
                / 6.0,
        )
}
