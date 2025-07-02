use std::{
    cmp::{self, Ordering},
    collections::HashSet,
    path::{Path, PathBuf},
    thread::available_parallelism,
};

use ffmpeg::format::Pixel;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::{
    broker::EncoderCrash,
    chunk::Chunk,
    metrics::{
        butteraugli::ButteraugliSubMetric,
        statistics::MetricStatistics,
        vmaf::{read_vmaf_file, run_vmaf, run_vmaf_weighted},
        xpsnr::{read_xpsnr_file, run_xpsnr, XPSNRSubMetric},
    },
    progress_bar::update_mp_msg,
    vapoursynth::{measure_butteraugli, measure_ssimulacra2, measure_xpsnr},
    Encoder,
    ProbingSpeed,
    ProbingStatistic,
    ProbingStatisticName,
    TargetMetric,
    VmafFeature,
};

const SCORE_TOLERANCE: f64 = 0.01;

/// Maximum squared sum of normalized derivatives for PCHIP monotonicity
/// constraint. If alpha^2 + beta^2 > 9, the derivatives are scaled down to
/// preserve monotonicity.
const PCHIP_MAX_TAU_SQUARED: f64 = 9.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetQuality {
    pub vmaf_res:              String,
    pub probe_res:             Option<String>,
    pub vmaf_scaler:           String,
    pub vmaf_filter:           Option<String>,
    pub vmaf_threads:          usize,
    pub model:                 Option<PathBuf>,
    pub probing_rate:          usize,
    pub probing_speed:         Option<ProbingSpeed>,
    pub probes:                u32,
    pub target:                f64,
    pub metric:                TargetMetric,
    pub min_q:                 u32,
    pub max_q:                 u32,
    pub encoder:               Encoder,
    pub pix_format:            Pixel,
    pub temp:                  String,
    pub workers:               usize,
    pub video_params:          Vec<String>,
    pub vspipe_args:           Vec<String>,
    pub probe_slow:            bool,
    pub probing_vmaf_features: Vec<VmafFeature>,
    pub probing_statistic:     ProbingStatistic,
}

impl TargetQuality {
    fn per_shot_target_quality(
        &self,
        chunk: &Chunk,
        worker_id: Option<usize>,
    ) -> anyhow::Result<u32> {
        // History of probe results as quantizer-score pairs
        let mut quantizer_score_history: Vec<(u32, f64)> = vec![];

        let update_progress_bar = |next_quantizer: u32| {
            if let Some(worker_id) = worker_id {
                update_mp_msg(
                    worker_id,
                    format!(
                        "Targeting {metric} Quality {target} - Testing {quantizer}",
                        metric = self.metric,
                        target = self.target,
                        quantizer = next_quantizer
                    ),
                );
            }
        };

        // Initialize quantizer limits from specified minimum and maximum quantizers
        let mut lower_quantizer_limit = self.min_q;
        let mut upper_quantizer_limit = self.max_q;

        loop {
            let next_quantizer = predict_quantizer(
                lower_quantizer_limit,
                upper_quantizer_limit,
                &quantizer_score_history,
                match self.metric {
                    // For inverse metrics, target must be inverted for ascending comparisons
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -self.target,
                    _ => self.target,
                },
            );

            if let Some((quantizer, score)) = quantizer_score_history
                .iter()
                .find(|(quantizer, _)| *quantizer == next_quantizer)
            {
                // Predicted quantizer has already been probed
                log_probes(
                    &quantizer_score_history,
                    self.metric,
                    self.target,
                    chunk.frames() as u32,
                    self.probing_rate as u32,
                    self.probing_speed,
                    &chunk.name(),
                    *quantizer,
                    match self.metric {
                        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                        _ => *score,
                    },
                    SkipProbingReason::None,
                );
                break;
            }

            update_progress_bar(next_quantizer);

            let score = {
                let value = self.probe(chunk, next_quantizer as usize)?;

                // Butteraugli is an inverse metric, invert score for comparisons
                match self.metric {
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -value,
                    _ => value,
                }
            };
            let score_within_tolerance = within_tolerance(
                match self.metric {
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                    _ => score,
                },
                self.target,
            );

            quantizer_score_history.push((next_quantizer, score));

            if score_within_tolerance || quantizer_score_history.len() >= self.probes as usize {
                log_probes(
                    &quantizer_score_history,
                    self.metric,
                    self.target,
                    chunk.frames() as u32,
                    self.probing_rate as u32,
                    self.probing_speed,
                    &chunk.name(),
                    next_quantizer,
                    match self.metric {
                        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                        _ => score,
                    },
                    if score_within_tolerance {
                        SkipProbingReason::WithinTolerance
                    } else {
                        SkipProbingReason::ProbeLimitReached
                    },
                );
                break;
            }

            if score
                > (match self.metric {
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -self.target,
                    _ => self.target,
                })
            {
                lower_quantizer_limit = (next_quantizer + 1).min(upper_quantizer_limit);
            } else {
                upper_quantizer_limit = (next_quantizer - 1).max(lower_quantizer_limit);
            }

            // Ensure quantizer limits are valid
            if lower_quantizer_limit > upper_quantizer_limit {
                log_probes(
                    &quantizer_score_history,
                    self.metric,
                    self.target,
                    chunk.frames() as u32,
                    self.probing_rate as u32,
                    self.probing_speed,
                    &chunk.name(),
                    next_quantizer,
                    match self.metric {
                        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                        _ => score,
                    },
                    if score
                        > (match self.metric {
                            TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => {
                                -self.target
                            },
                            _ => self.target,
                        })
                    {
                        SkipProbingReason::QuantizerTooHigh
                    } else {
                        SkipProbingReason::QuantizerTooLow
                    },
                );
                break;
            }
        }

        let final_quantizer_score = if let Some(highest_quantizer_score_within_tolerance) =
            quantizer_score_history
                .iter()
                .filter(|(_, score)| {
                    within_tolerance(
                        match self.metric {
                            TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                            _ => *score,
                        },
                        self.target,
                    )
                })
                .max_by_key(|(quantizer, _)| *quantizer)
        {
            // Multiple probes within tolerance, choose the highest
            highest_quantizer_score_within_tolerance
        } else {
            // No quantizers within tolerance, choose the quantizer closest to target
            quantizer_score_history
                .iter()
                .min_by(|(_, score1), (_, score2)| {
                    let score_1 = match self.metric {
                        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score1,
                        _ => *score1,
                    };
                    let score_2 = match self.metric {
                        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score2,
                        _ => *score2,
                    };
                    let difference1 = (score_1 - self.target).abs();
                    let difference2 = (score_2 - self.target).abs();
                    difference1.partial_cmp(&difference2).unwrap_or(Ordering::Equal)
                })
                .unwrap()
        };

        // Note: if the score is to be returned in the future, ensure to invert it back
        // if metric is inverse (eg. Butteraugli)
        Ok(final_quantizer_score.0)
    }

    fn probe(&self, chunk: &Chunk, quantizer: usize) -> anyhow::Result<f64> {
        let probe_name = self.encode_probe(chunk, quantizer)?;

        let aggregate_frame_scores = |scores: Vec<f64>| -> anyhow::Result<f64> {
            let mut statistics = MetricStatistics::new(scores);

            let aggregate = match self.probing_statistic.name {
                ProbingStatisticName::Automatic => {
                    if self.metric == TargetMetric::VMAF {
                        // Preserve legacy VMAF aggregation
                        return Ok(statistics.percentile(1));
                    }

                    let sigma_1 = {
                        let sigma_distance = statistics.standard_deviation();
                        let statistic = statistics.mean() - sigma_distance;
                        statistic.clamp(statistics.minimum(), statistics.maximum())
                    };

                    // Based on probing speed and quantizer
                    // Probing slower leads to more accurate scores (lower variance)
                    // Lower quantizer leads to more accurate scores (lower variance) (needs
                    // testing)
                    match self.probing_speed.unwrap_or(ProbingSpeed::VeryFast) {
                        ProbingSpeed::VerySlow | ProbingSpeed::Slow => {
                            if self.encoder.get_cq_relative_percentage(quantizer) < 0.75 {
                                // Conservative: Use outliers to determine aggregate
                                statistics.percentile(1)
                            } else {
                                // Less conservative: Use -1 sigma to determine aggregate
                                sigma_1
                            }
                        },
                        _ => {
                            if self.encoder.get_cq_relative_percentage(quantizer) > 0.25 {
                                // Liberal: Use mean to determine aggregate
                                statistics.mean()
                            } else {
                                // Less liberal: Use -1 sigma to determine aggregate
                                sigma_1
                            }
                        },
                    }
                },
                ProbingStatisticName::Mean => statistics.mean(),
                ProbingStatisticName::RootMeanSquare => statistics.root_mean_square(),
                ProbingStatisticName::Median => statistics.median(),
                ProbingStatisticName::Harmonic => statistics.harmonic_mean(),
                ProbingStatisticName::Percentile => {
                    let value = self
                        .probing_statistic
                        .value
                        .ok_or(anyhow::anyhow!("Percentile statistic requires a value"))?;
                    statistics.percentile(value as usize)
                },
                ProbingStatisticName::StandardDeviation => {
                    let value = self.probing_statistic.value.ok_or(anyhow::anyhow!(
                        "Standard deviation statistic requires a value"
                    ))?;
                    let sigma_distance = value * statistics.standard_deviation();
                    let statistic = statistics.mean() + sigma_distance;
                    statistic.clamp(statistics.minimum(), statistics.maximum())
                },
                ProbingStatisticName::Mode => statistics.mode(),
                ProbingStatisticName::Minimum => statistics.minimum(),
                ProbingStatisticName::Maximum => statistics.maximum(),
            };

            Ok(aggregate)
        };

        match self.metric {
            TargetMetric::VMAF => {
                let features: HashSet<_> = self.probing_vmaf_features.iter().copied().collect();
                let use_weighted = features.contains(&VmafFeature::Weighted);
                let use_neg = features.contains(&VmafFeature::Neg);
                let use_uhd = features.contains(&VmafFeature::Uhd);
                let disable_motion = features.contains(&VmafFeature::Motionless);

                let default_model = match (use_uhd, use_neg) {
                    (true, true) => Some(PathBuf::from("vmaf_4k_v0.6.1neg.json")),
                    (true, false) => Some(PathBuf::from("vmaf_4k_v0.6.1.json")),
                    (false, true) => Some(PathBuf::from("vmaf_v0.6.1neg.json")),
                    (false, false) => None,
                };

                let model = if self.model.is_none() {
                    default_model.as_ref()
                } else {
                    self.model.as_ref()
                };

                let vmaf_scores = if use_weighted {
                    run_vmaf_weighted(
                        &probe_name,
                        chunk.source_cmd.as_slice(),
                        self.vspipe_args.clone(),
                        model,
                        self.probe_res.as_ref().unwrap_or(&self.vmaf_res),
                        &self.vmaf_scaler,
                        self.probing_rate,
                        self.vmaf_filter.as_deref(),
                        self.vmaf_threads,
                        chunk.frame_rate,
                        disable_motion,
                    )
                    .map_err(|e| {
                        Box::new(EncoderCrash {
                            exit_status:        std::process::ExitStatus::default(),
                            source_pipe_stderr: String::new().into(),
                            ffmpeg_pipe_stderr: None,
                            stderr:             format!("VMAF calculation failed: {e}").into(),
                            stdout:             String::new().into(),
                        })
                    })?
                } else {
                    let fl_path = std::path::Path::new(&chunk.temp)
                        .join("split")
                        .join(format!("{index}.json", index = chunk.index));

                    run_vmaf(
                        &probe_name,
                        chunk.source_cmd.as_slice(),
                        self.vspipe_args.clone(),
                        &fl_path,
                        model,
                        self.probe_res.as_ref().unwrap_or(&self.vmaf_res),
                        &self.vmaf_scaler,
                        self.probing_rate,
                        self.vmaf_filter.as_deref(),
                        self.vmaf_threads,
                        chunk.frame_rate,
                        disable_motion,
                    )?;

                    read_vmaf_file(&fl_path)?
                };

                aggregate_frame_scores(vmaf_scores)
            },
            TargetMetric::SSIMULACRA2 => {
                let scores = measure_ssimulacra2(
                    &chunk.input,
                    &probe_name,
                    (chunk.start_frame as u32, chunk.end_frame as u32),
                    self.probe_res.as_ref(),
                    self.probing_rate,
                )?;

                aggregate_frame_scores(scores)
            },
            TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => {
                let scores = measure_butteraugli(
                    match self.metric {
                        TargetMetric::ButteraugliINF => ButteraugliSubMetric::InfiniteNorm,
                        TargetMetric::Butteraugli3 => ButteraugliSubMetric::ThreeNorm,
                        _ => unreachable!(),
                    },
                    &chunk.input,
                    &probe_name,
                    (chunk.start_frame as u32, chunk.end_frame as u32),
                    self.probe_res.as_ref(),
                    self.probing_rate,
                )?;

                aggregate_frame_scores(scores)
            },
            TargetMetric::XPSNR | TargetMetric::XPSNRWeighted => {
                let submetric = if self.metric == TargetMetric::XPSNR {
                    XPSNRSubMetric::Minimum
                } else {
                    XPSNRSubMetric::Weighted
                };
                if self.probing_rate > 1 {
                    let scores = measure_xpsnr(
                        submetric,
                        &chunk.input,
                        &probe_name,
                        (chunk.start_frame as u32, chunk.end_frame as u32),
                        self.probe_res.as_ref(),
                        self.probing_rate,
                    )?;

                    aggregate_frame_scores(scores)
                } else {
                    let fl_path =
                        Path::new(&chunk.temp).join("split").join(format!("{}.json", chunk.index));

                    run_xpsnr(
                        &probe_name,
                        chunk.source_cmd.as_slice(),
                        self.vspipe_args.clone(),
                        &fl_path,
                        self.probe_res.as_ref().unwrap_or(&self.vmaf_res),
                        &self.vmaf_scaler,
                        self.probing_rate,
                        chunk.frame_rate,
                    )?;

                    let (aggregate, scores) = read_xpsnr_file(fl_path, submetric);

                    match self.probing_statistic.name {
                        ProbingStatisticName::Automatic => Ok(aggregate),
                        _ => aggregate_frame_scores(scores),
                    }
                }
            },
        }
    }

    fn encode_probe(&self, chunk: &Chunk, q: usize) -> Result<PathBuf, Box<EncoderCrash>> {
        let vmaf_threads = if self.vmaf_threads == 0 {
            vmaf_auto_threads(self.workers)
        } else {
            self.vmaf_threads
        };

        let cmd = self.encoder.probe_cmd(
            self.temp.clone(),
            chunk.index,
            q,
            self.pix_format,
            self.probing_rate,
            self.probing_speed,
            vmaf_threads,
            self.video_params.clone(),
            self.probe_slow,
        );

        let future = async {
            let source_cmd = chunk.source_cmd.clone();
            let cmd = cmd.clone();

            tokio::task::spawn_blocking(move || {
                let mut source = if let [pipe_cmd, args @ ..] = &*source_cmd {
                    std::process::Command::new(pipe_cmd)
                        .args(args)
                        .stderr(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| EncoderCrash {
                            exit_status:        std::process::ExitStatus::default(),
                            source_pipe_stderr: format!("Failed to spawn source: {e}").into(),
                            ffmpeg_pipe_stderr: None,
                            stderr:             String::new().into(),
                            stdout:             String::new().into(),
                        })?
                } else {
                    unreachable!()
                };

                let source_stdout = source.stdout.take().unwrap();

                let mut source_pipe = if let [ffmpeg, args @ ..] = &*cmd.0 {
                    std::process::Command::new(ffmpeg)
                        .args(args)
                        .stdin(source_stdout)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| EncoderCrash {
                            exit_status:        std::process::ExitStatus::default(),
                            source_pipe_stderr: format!("Failed to spawn ffmpeg: {e}").into(),
                            ffmpeg_pipe_stderr: None,
                            stderr:             String::new().into(),
                            stdout:             String::new().into(),
                        })?
                } else {
                    unreachable!()
                };

                let source_pipe_stdout = source_pipe.stdout.take().unwrap();

                let mut enc_pipe = if let [cmd, args @ ..] = &*cmd.1 {
                    std::process::Command::new(cmd.as_ref())
                        .args(args.iter().map(AsRef::as_ref))
                        .stdin(source_pipe_stdout)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| EncoderCrash {
                            exit_status:        std::process::ExitStatus::default(),
                            source_pipe_stderr: String::new().into(),
                            ffmpeg_pipe_stderr: None,
                            stderr:             format!("Failed to spawn encoder: {e}").into(),
                            stdout:             String::new().into(),
                        })?
                } else {
                    unreachable!()
                };

                // Drop stdout to prevent buffer deadlock
                drop(enc_pipe.stdout.take());

                // Start reading stderr concurrently to prevent deadlock
                use std::{io::Read, thread};

                let source_stderr = source.stderr.take().unwrap();
                let source_pipe_stderr = source_pipe.stderr.take().unwrap();
                let enc_stderr = enc_pipe.stderr.take().unwrap();

                let stderr_thread1 = thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut stderr = source_stderr;
                    stderr.read_to_end(&mut buf).ok();
                    buf
                });

                let stderr_thread2 = thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut stderr = source_pipe_stderr;
                    stderr.read_to_end(&mut buf).ok();
                    buf
                });

                let stderr_thread3 = thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut stderr = enc_stderr;
                    stderr.read_to_end(&mut buf).ok();
                    buf
                });

                // Wait for encoder & other processes to finish
                let enc_status = enc_pipe.wait().map_err(|e| EncoderCrash {
                    exit_status:        std::process::ExitStatus::default(),
                    source_pipe_stderr: String::new().into(),
                    ffmpeg_pipe_stderr: None,
                    stderr:             format!("Failed to wait for encoder: {e}").into(),
                    stdout:             String::new().into(),
                })?;

                let _ = source_pipe.wait();
                let _ = source.wait();

                // Collect stderr after process finishes
                let stderr_handles = (
                    stderr_thread1.join().unwrap_or_default(),
                    stderr_thread2.join().unwrap_or_default(),
                    stderr_thread3.join().unwrap_or_default(),
                );

                if !enc_status.success() {
                    return Err(EncoderCrash {
                        exit_status:        enc_status,
                        source_pipe_stderr: stderr_handles.0.into(),
                        ffmpeg_pipe_stderr: Some(stderr_handles.1.into()),
                        stderr:             stderr_handles.2.into(),
                        stdout:             String::new().into(),
                    });
                }

                Ok(())
            })
            .await
            .unwrap()
        };

        let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();
        rt.block_on(future)?;

        let extension = match self.encoder {
            crate::encoder::Encoder::x264 => "264",
            crate::encoder::Encoder::x265 => "hevc",
            _ => "ivf",
        };

        let probe_name = std::path::Path::new(&chunk.temp)
            .join("split")
            .join(format!("v_{index:05}_{q}.{extension}", index = chunk.index));

        Ok(probe_name)
    }

    #[inline]
    pub fn per_shot_target_quality_routine(
        &self,
        chunk: &mut Chunk,
        worker_id: Option<usize>,
    ) -> anyhow::Result<()> {
        chunk.tq_cq = Some(self.per_shot_target_quality(chunk, worker_id)?);
        Ok(())
    }
}

fn linear_interpolate(x: &[f64; 2], y: &[f64; 2], xi: f64) -> Option<f64> {
    // Check strictly increasing
    if x[1] <= x[0] {
        return None;
    }

    // Linear interpolation formula: y = y0 + (y1 - y0) * (xi - x0) / (x1 - x0)
    let t = (xi - x[0]) / (x[1] - x[0]);
    Some(y[0] + t * (y[1] - y[0]))
}

fn natural_cubic_spline(x: &[f64], y: &[f64], xi: f64) -> Option<f64> {
    let n = x.len();
    if n < 3 || n != y.len() {
        return None;
    }

    // Noramally, no bounds check is needed - we're interpolating, not extrapolating
    // The target (xi) is a score value we're looking for, not restricted to input
    // range

    // Verify xi is within the observed range (it should be by algorithm design)
    if xi < x[0] || xi > x[n - 1] {
        trace!(
            "Natural cubic spline: unexpected extrapolation case - xi = {xi}, range = [{}, {}]",
            x[0],
            x[n - 1]
        );
        return None;
    }

    // Calculate intervals
    let mut h = vec![0.0; n - 1];
    for i in 0..n - 1 {
        h[i] = x[i + 1] - x[i];
        if h[i] <= 0.0 {
            trace!(
                "Natural cubic spline: x values not strictly increasing at index {i}: {prev} >= \
                 {next}",
                prev = x[i],
                next = x[i + 1]
            );
            return None; // x must be strictly increasing
        }
    }

    // Set up tridiagonal system for second derivatives
    let mut a = vec![0.0; n];
    let mut b = vec![2.0; n];
    let mut c = vec![0.0; n];
    let mut d = vec![0.0; n];

    // Natural boundary conditions: second derivative = 0 at endpoints
    b[0] = 1.0;
    b[n - 1] = 1.0;

    // Interior points
    for i in 1..n - 1 {
        a[i] = h[i - 1];
        b[i] = 2.0 * (h[i - 1] + h[i]);
        c[i] = h[i];
        d[i] = 3.0 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
    }

    // Solve tridiagonal system (Thomas algorithm)
    let mut m = vec![0.0; n];
    let mut l = vec![0.0; n];
    let mut z = vec![0.0; n];

    l[0] = b[0];
    if l[0] == 0.0 {
        trace!("Natural cubic spline: Singular matrix at first step");
        return None;
    }
    for i in 1..n {
        l[i] = b[i] - a[i] * c[i - 1] / l[i - 1];
        if l[i] == 0.0 {
            trace!("Natural cubic spline: Singular matrix at step {i}");
            return None;
        }
        z[i] = (d[i] - a[i] * z[i - 1]) / l[i];
    }

    m[n - 1] = z[n - 1];
    for i in (0..n - 1).rev() {
        m[i] = z[i] - c[i] * m[i + 1] / l[i];
    }

    // Find the interval containing xi
    let mut k = 0;
    for i in 0..n - 1 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Evaluate cubic polynomial
    let dx = xi - x[k];
    let h_k = h[k];

    let a_coeff = y[k];
    let b_coeff = (y[k + 1] - y[k]) / h_k - h_k * (2.0 * m[k] + m[k + 1]) / 3.0;
    let c_coeff = m[k];
    let d_coeff = (m[k + 1] - m[k]) / (3.0 * h_k);

    Some(a_coeff + b_coeff * dx + c_coeff * dx * dx + d_coeff * dx * dx * dx)
}

fn pchip_interpolate(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Find interval containing xi
    let mut k = 0;
    for i in 0..3 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Calculate slopes
    let s0 = (y[1] - y[0]) / (x[1] - x[0]);
    let s1 = (y[2] - y[1]) / (x[2] - x[1]);
    let s2 = (y[3] - y[2]) / (x[3] - x[2]);

    // Calculate derivatives using PCHIP method
    let mut d = [0.0; 4];

    // Endpoint derivatives
    d[0] = s0;
    d[3] = s2;

    // Interior derivatives (weighted harmonic mean)
    #[allow(clippy::needless_range_loop)]
    for i in 1..=2 {
        let (s_prev, s_next, h_prev, h_next) = if i == 1 {
            (s0, s1, x[1] - x[0], x[2] - x[1])
        } else {
            (s1, s2, x[2] - x[1], x[3] - x[2])
        };

        if s_prev * s_next <= 0.0 {
            d[i] = 0.0;
        } else {
            let w1 = 2.0 * h_next + h_prev;
            let w2 = h_next + 2.0 * h_prev;
            d[i] = (w1 + w2) / (w1 / s_prev + w2 / s_next);
        }
    }

    // Monotonicity constraint
    let slopes = [s0, s1, s2];
    for i in 0..3 {
        if slopes[i] == 0.0 {
            d[i] = 0.0;
            d[i + 1] = 0.0;
        } else {
            let alpha = d[i] / slopes[i];
            let beta = d[i + 1] / slopes[i];
            let tau = alpha * alpha + beta * beta;

            if tau > PCHIP_MAX_TAU_SQUARED {
                let scale = 3.0 / tau.sqrt();
                d[i] = scale * alpha * slopes[i];
                d[i + 1] = scale * beta * slopes[i];
            }
        }
    }

    // Hermite cubic evaluation
    let h = x[k + 1] - x[k];
    let t = (xi - x[k]) / h;
    let t2 = t * t;
    let t3 = t2 * t;

    Some(
        (2.0 * t3 - 3.0 * t2 + 1.0) * y[k]
            + (t3 - 2.0 * t2 + t) * h * d[k]
            + (-2.0 * t3 + 3.0 * t2) * y[k + 1]
            + (t3 - t2) * h * d[k + 1],
    )
}

fn predict_quantizer(
    lower_quantizer_limit: u32,
    upper_quantizer_limit: u32,
    quantizer_score_history: &[(u32, f64)],
    target: f64,
) -> u32 {
    // The midpoint between the upper and lower quantizer bounds
    let binary_search = (lower_quantizer_limit + upper_quantizer_limit) / 2;

    let predicted_quantizer = match quantizer_score_history.len() {
        0..=1 => binary_search as f64,
        _ => {
            // Sort history by quantizer
            let mut sorted_quantizer_score_history = quantizer_score_history.to_vec();
            sorted_quantizer_score_history.sort_by(|(_, score1), (_, score2)| {
                match score1.partial_cmp(score2) {
                    Some(ordering) => ordering,
                    None => {
                        trace!("Warning: NaN encountered in score comparison");
                        std::cmp::Ordering::Equal
                    },
                }
            });

            match sorted_quantizer_score_history.len() {
                2 => {
                    // 3rd probe: linear interpolation
                    let scores =
                        [sorted_quantizer_score_history[0].1, sorted_quantizer_score_history[1].1];
                    let quantizers = [
                        sorted_quantizer_score_history[0].0 as f64,
                        sorted_quantizer_score_history[1].0 as f64,
                    ];

                    linear_interpolate(&scores, &quantizers, target).unwrap_or_else(|| {
                        trace!("Linear interpolation failed, falling back to binary search");
                        binary_search as f64
                    })
                },
                3 => {
                    // 4th probe: natural cubic spline
                    let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                        sorted_quantizer_score_history.iter().map(|(q, s)| (*s, *q as f64)).unzip();

                    natural_cubic_spline(&scores, &quantizers, target).unwrap_or_else(|| {
                        trace!("Natural cubic spline failed, falling back to binary search");
                        binary_search as f64
                    })
                },
                4 => {
                    // 5th probe: PCHIP interpolation
                    let (scores, quantizers): ([f64; 4], [f64; 4]) = {
                        let mut s = [0.0; 4];
                        let mut q = [0.0; 4];
                        for (i, (quantizer, score)) in
                            sorted_quantizer_score_history.iter().enumerate()
                        {
                            s[i] = *score;
                            q[i] = *quantizer as f64;
                        }
                        (s, q)
                    };

                    pchip_interpolate(&scores, &quantizers, target).unwrap_or_else(|| {
                        trace!("PCHIP interpolation failed, falling back to binary search");
                        binary_search as f64
                    })
                },
                _ => binary_search as f64, // 6+ probes: binary search only
            }
        },
    };

    // Ensure predicted quantizer is an integer and within bounds
    (predicted_quantizer.round() as u32).clamp(lower_quantizer_limit, upper_quantizer_limit)
}

fn within_tolerance(score: f64, target: f64) -> bool {
    (score - target).abs() / target < SCORE_TOLERANCE
}

pub fn vmaf_auto_threads(workers: usize) -> usize {
    const OVER_PROVISION_FACTOR: f64 = 1.25;

    let threads = available_parallelism()
        .expect("Unrecoverable: Failed to get thread count")
        .get();

    cmp::max(
        ((threads / workers) as f64 * OVER_PROVISION_FACTOR) as usize,
        1,
    )
}

#[derive(Copy, Clone)]
pub enum SkipProbingReason {
    QuantizerTooHigh,
    QuantizerTooLow,
    WithinTolerance,
    ProbeLimitReached,
    None,
}

#[allow(clippy::too_many_arguments)]
pub fn log_probes(
    quantizer_score_history: &[(u32, f64)],
    metric: TargetMetric,
    target: f64,
    frames: u32,
    probing_rate: u32,
    probing_speed: Option<ProbingSpeed>,
    chunk_name: &str,
    target_quantizer: u32,
    target_score: f64,
    skip: SkipProbingReason,
) {
    // Sort history by quantizer
    let mut sorted_quantizer_scores = quantizer_score_history.to_vec();
    sorted_quantizer_scores.sort_by_key(|(quantizer, _)| *quantizer);
    // Butteraugli is an inverse metric and needs to be inverted back before display
    if matches!(
        metric,
        TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3
    ) {
        sorted_quantizer_scores = sorted_quantizer_scores
            .iter()
            .map(|(quantizer, score)| (*quantizer, -score))
            .collect();
    }

    debug!(
        "chunk {name}: Target={target}, Metric={target_metric}, P-Rate={rate}, P-Speed={speed:?}, \
         {frame_count} frames
        TQ-Probes: {history:.2?}{suffix}
        Final Q={target_quantizer:.0}, Final Score={target_score:.2}",
        name = chunk_name,
        target = target,
        target_metric = metric,
        rate = probing_rate,
        speed = probing_speed.unwrap_or(ProbingSpeed::VeryFast),
        frame_count = frames,
        history = sorted_quantizer_scores,
        suffix = match skip {
            SkipProbingReason::None => "",
            SkipProbingReason::QuantizerTooHigh => "Early Skip High Quantizer",
            SkipProbingReason::QuantizerTooLow => " Early Skip Low Quantizer",
            SkipProbingReason::WithinTolerance => " Early Skip Within Tolerance",
            SkipProbingReason::ProbeLimitReached => " Early Skip Probe Limit Reached",
        },
        target_quantizer = target_quantizer,
        target_score = target_score
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_linear_interpolate() {
        // Test basic linear interpolation using real CRF/score data
        let x = [82.502861, 87.600777]; // scores (ascending order)
        let y = [20.0, 10.0]; // CRFs

        // Test exact points
        assert_eq!(linear_interpolate(&x, &y, 82.502861), Some(20.0));
        assert_eq!(linear_interpolate(&x, &y, 87.600777), Some(10.0));

        // Test midpoint - score 85.051819 should give CRF ~15
        assert!((linear_interpolate(&x, &y, 85.051819).unwrap() - 15.0).abs() < 0.1);

        // Test interpolation for score 84.0
        let result = linear_interpolate(&x, &y, 84.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 15.0 && result.unwrap() < 20.0);

        let x2 = [78.737953, 89.179634]; // scores (ascending order)
        let y2 = [15.0, 5.0]; // CRFs
        assert!((linear_interpolate(&x2, &y2, 83.958794).unwrap() - 10.0).abs() < 0.1);

        // Test non-increasing x values (should return None)
        let x_bad = [87.600777, 82.502861]; // Not ascending
        let y_bad = [10.0, 20.0];
        assert_eq!(linear_interpolate(&x_bad, &y_bad, 85.0), None);

        // Test equal x values (should return None)
        let x_equal = [85.0, 85.0];
        assert_eq!(linear_interpolate(&x_equal, &y, 85.0), None);
    }

    #[test]
    fn test_natural_cubic_spline() {
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = vec![72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = vec![30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!((natural_cubic_spline(&x, &y, 72.812233).unwrap() - 30.0).abs() < 1e-10);
        assert!((natural_cubic_spline(&x, &y, 78.517479).unwrap() - 20.0).abs() < 1e-10);
        assert!((natural_cubic_spline(&x, &y, 84.872162).unwrap() - 10.0).abs() < 1e-10);

        // Test interpolation for score 81.0
        let result = natural_cubic_spline(&x, &y, 81.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 10.0 && result.unwrap() < 20.0);

        // CRF 15 (84.864449), CRF 25 (80.161186), CRF 35 (72.134048)
        let x2 = vec![72.134048, 80.161186, 84.864449]; // scores (ascending order)
        let y2 = vec![35.0, 25.0, 15.0]; // CRFs

        // Test interpolation for score 82.0
        let result = natural_cubic_spline(&x2, &y2, 82.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 15.0 && result.unwrap() < 25.0);

        // CRF 20 (83.0155), CRF 30 (77.7812), CRF 40 (67.3447)
        let x3 = vec![67.3447, 77.7812, 83.0155]; // scores (ascending order)
        let y3 = vec![40.0, 30.0, 20.0]; // CRFs

        // Test interpolation for score 80.0
        let result = natural_cubic_spline(&x3, &y3, 80.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 20.0 && result.unwrap() < 30.0);

        // Test with non-increasing x values (should return None)
        let x_bad = vec![84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = vec![10.0, 20.0, 25.0];
        assert_eq!(natural_cubic_spline(&x_bad, &y_bad, 79.0), None);

        // Test with too few points (should return None)
        let x_short = vec![87.0715, 90.0064];
        let y_short = vec![20.0, 10.0];
        assert_eq!(natural_cubic_spline(&x_short, &y_short, 88.0), None);

        // Test with mismatched lengths (should return None)
        let x_mismatch = vec![83.8005, 87.0715, 90.0064];
        let y_mismatch = vec![30.0, 20.0];
        assert_eq!(natural_cubic_spline(&x_mismatch, &y_mismatch, 85.0), None);
    }

    #[test]
    fn test_pchip_interpolate() {
        // Test with monotonic data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!((pchip_interpolate(&x, &y, 72.9709).unwrap() - 35.0).abs() < 1e-10);
        assert!((pchip_interpolate(&x, &y, 80.5088).unwrap() - 25.0).abs() < 1e-10);
        assert!((pchip_interpolate(&x, &y, 85.7452).unwrap() - 15.0).abs() < 1e-10);
        assert!((pchip_interpolate(&x, &y, 92.4354).unwrap() - 5.0).abs() < 1e-10);

        // Test interpolation for score 89.0
        let result = pchip_interpolate(&x, &y, 89.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 5.0 && result.unwrap() < 15.0);

        // Test with data that has varying slopes
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Should handle the steep changes in score
        let result = pchip_interpolate(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(result.unwrap() > 45.0 && result.unwrap() < 50.0);

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(pchip_interpolate(&x_bad, &y_bad, 87.0), None);

        // Test edge case with nearly flat region
        // CRF 63-66 have very similar scores
        let x_flat = [4.944567, 5.270722, 5.345044, 5.575547]; // scores (ascending order)
        let y_flat = [65.0, 66.0, 64.0, 63.0]; // CRFs
        let result = pchip_interpolate(&x_flat, &y_flat, 5.1);
        assert!(result.is_some());
        // Should handle the nearly flat region gracefully
    }

    // Full algorithm simulation tests
    fn get_score_map(case: usize) -> HashMap<u32, f64> {
        match case {
            1 => [(35, 80.08)].iter().cloned().collect(),
            2 => [(17, 80.03), (35, 65.73)].iter().cloned().collect(),
            3 => [(17, 83.15), (22, 80.02), (35, 71.94)].iter().cloned().collect(),
            4 => [(17, 85.81), (30, 80.92), (32, 80.01), (35, 78.05)].iter().cloned().collect(),
            5 => [(35, 83.31), (53, 81.22), (55, 80.03), (61, 73.56), (64, 67.56)]
                .iter()
                .cloned()
                .collect(),
            6 => [
                (35, 86.99),
                (53, 84.41),
                (57, 82.47),
                (59, 81.14),
                (60, 80.09),
                (61, 78.58),
                (69, 68.57),
                (70, 64.90),
            ]
            .iter()
            .cloned()
            .collect(),
            _ => panic!("Unknown case"),
        }
    }

    fn run_av1an_simulation(case: usize) -> Vec<(u32, f64)> {
        let scores = get_score_map(case);
        let mut history = vec![];
        let mut lo = 1u32;
        let mut hi = 70u32;
        let target = 80.0;

        for _ in 1..=10 {
            let next_quantizer = predict_quantizer(lo, hi, &history, target);

            // Check if this quantizer was already probed
            if let Some((_quantizer, _score)) =
                history.iter().find(|(quantizer, _)| *quantizer == next_quantizer)
            {
                break;
            }

            if let Some(&score) = scores.get(&next_quantizer) {
                history.push((next_quantizer, score));

                if within_tolerance(score, target) {
                    break;
                }

                if score > target {
                    lo = lo.max(next_quantizer + 1);
                } else {
                    hi = hi.min(next_quantizer.saturating_sub(1));
                }
            } else {
                break;
            }
        }

        history
    }

    #[test]
    fn test_all_cases() {
        for case in 1..=6 {
            let result = run_av1an_simulation(case);
            assert!(!result.is_empty());
            assert!(within_tolerance(result.last().unwrap().1, 80.0));
        }
    }
}
