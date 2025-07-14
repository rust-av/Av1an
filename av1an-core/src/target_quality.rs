use std::{
    borrow::Cow,
    cmp::{self, Ordering},
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    str::FromStr,
    thread::{self, available_parallelism},
};

use anyhow::bail;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::{
    broker::EncoderCrash,
    chunk::Chunk,
    ffmpeg::FFPixelFormat,
    interpol::{
        akima_interpolate,
        catmull_rom_interpolate,
        cubic_polynomial_interpolate,
        linear_interpolate,
        natural_cubic_spline,
        pchip_interpolate,
        quadratic_interpolate,
    },
    metrics::{
        butteraugli::ButteraugliSubMetric,
        statistics::MetricStatistics,
        vmaf::{read_vmaf_file, run_vmaf, run_vmaf_weighted},
        xpsnr::{read_xpsnr_file, run_xpsnr, XPSNRSubMetric},
    },
    progress_bar::update_mp_msg,
    vapoursynth::{measure_butteraugli, measure_ssimulacra2, measure_xpsnr, VapoursynthPlugins},
    Encoder,
    ProbingSpeed,
    ProbingStatistic,
    ProbingStatisticName,
    TargetMetric,
    VmafFeature,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpolationMethod {
    Linear,
    Quadratic,
    Natural,
    Pchip,
    Catmull,
    Akima,
    CubicPolynomial,
}

impl FromStr for InterpolationMethod {
    type Err = ();

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "linear" => Ok(Self::Linear),
            "quadratic" => Ok(Self::Quadratic),
            "natural" => Ok(Self::Natural),
            "pchip" => Ok(Self::Pchip),
            "catmull" => Ok(Self::Catmull),
            "akima" => Ok(Self::Akima),
            "cubicpolynomial" | "cubic" => Ok(Self::CubicPolynomial),
            _ => Err(()),
        }
    }
}

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
    pub target:                (f64, f64),
    pub metric:                TargetMetric,
    pub min_q:                 u32,
    pub max_q:                 u32,
    pub interp_method:         Option<(InterpolationMethod, InterpolationMethod)>,
    pub encoder:               Encoder,
    pub pix_format:            FFPixelFormat,
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
        plugins: Option<&VapoursynthPlugins>,
    ) -> anyhow::Result<u32> {
        // History of probe results as quantizer-score pairs
        let mut quantizer_score_history: Vec<(u32, f64)> = vec![];

        let update_progress_bar = |next_quantizer: u32| {
            if let Some(worker_id) = worker_id {
                update_mp_msg(
                    worker_id,
                    format!(
                        "Targeting {metric} Quality {min}-{max} - Testing {quantizer}",
                        metric = self.metric,
                        min = self.target.0,
                        max = self.target.1,
                        quantizer = next_quantizer
                    ),
                );
            }
        };

        // Initialize quantizer limits from specified range or encoder defaults
        let mut lower_quantizer_limit = self.min_q;
        let mut upper_quantizer_limit = self.max_q;

        loop {
            let next_quantizer = predict_quantizer(
                lower_quantizer_limit,
                upper_quantizer_limit,
                &quantizer_score_history,
                match self.metric {
                    // For inverse metrics, target must be inverted for ascending comparisons
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => {
                        let (min, max) = self.target;
                        (-max, -min)
                    },
                    _ => self.target,
                },
                self.interp_method,
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
                let value = self.probe(chunk, next_quantizer as usize, plugins)?;

                // Butteraugli is an inverse metric, invert score for comparisons
                match self.metric {
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -value,
                    _ => value,
                }
            };
            let score_within_range = within_range(
                match self.metric {
                    TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => -score,
                    _ => score,
                },
                self.target,
            );

            quantizer_score_history.push((next_quantizer, score));

            if score_within_range || quantizer_score_history.len() >= self.probes as usize {
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
                    if score_within_range {
                        SkipProbingReason::WithinTolerance
                    } else {
                        SkipProbingReason::ProbeLimitReached
                    },
                );
                break;
            }

            let target_range = match self.metric {
                TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => {
                    (-self.target.1, -self.target.0)
                },
                _ => self.target,
            };

            if score > target_range.1 {
                lower_quantizer_limit = (next_quantizer + 1).min(upper_quantizer_limit);
            } else if score < target_range.0 {
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
                    if score > target_range.1 {
                        SkipProbingReason::QuantizerTooHigh
                    } else {
                        SkipProbingReason::QuantizerTooLow
                    },
                );
                break;
            }
        }

        let final_quantizer_score = if let Some(highest_quantizer_score_within_range) =
            quantizer_score_history
                .iter()
                .filter(|(_, score)| {
                    within_range(
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
            highest_quantizer_score_within_range
        } else {
            // No quantizers within tolerance, choose the quantizer closest to target
            let target_midpoint = (self.target.0 + self.target.1) / 2.0;
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
                    let difference1 = (score_1 - target_midpoint).abs();
                    let difference2 = (score_2 - target_midpoint).abs();
                    difference1.partial_cmp(&difference2).unwrap_or(Ordering::Equal)
                })
                .unwrap()
        };

        // Note: if the score is to be returned in the future, ensure to invert it back
        // if metric is inverse (eg. Butteraugli)
        Ok(final_quantizer_score.0)
    }

    fn probe(
        &self,
        chunk: &Chunk,
        quantizer: usize,
        plugins: Option<&VapoursynthPlugins>,
    ) -> anyhow::Result<f64> {
        let probe_name = self.encode_probe(chunk, quantizer)?;
        let reference_pipe_cmd = if let Some(proxy_cmd) = &chunk.proxy_cmd {
            proxy_cmd.as_slice()
        } else {
            chunk.source_cmd.as_slice()
        };

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
                        reference_pipe_cmd,
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
                        reference_pipe_cmd,
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
                let scores = if let Some(plugins) = plugins {
                    measure_ssimulacra2(
                        chunk.proxy.as_ref().unwrap_or(&chunk.input),
                        &probe_name,
                        (chunk.start_frame as u32, chunk.end_frame as u32),
                        self.probe_res.as_ref(),
                        self.probing_rate,
                        plugins,
                    )?
                } else {
                    bail!("SSIMULACRA2 requires Vapoursynth to be installed");
                };

                aggregate_frame_scores(scores)
            },
            TargetMetric::ButteraugliINF | TargetMetric::Butteraugli3 => {
                let scores = if let Some(plugins) = plugins {
                    measure_butteraugli(
                        match self.metric {
                            TargetMetric::ButteraugliINF => ButteraugliSubMetric::InfiniteNorm,
                            TargetMetric::Butteraugli3 => ButteraugliSubMetric::ThreeNorm,
                            _ => unreachable!(),
                        },
                        chunk.proxy.as_ref().unwrap_or(&chunk.input),
                        &probe_name,
                        (chunk.start_frame as u32, chunk.end_frame as u32),
                        self.probe_res.as_ref(),
                        self.probing_rate,
                        plugins,
                    )?
                } else {
                    bail!("Butteraugli requires Vapoursynth to be installed");
                };

                aggregate_frame_scores(scores)
            },
            TargetMetric::XPSNR | TargetMetric::XPSNRWeighted => {
                let submetric = if self.metric == TargetMetric::XPSNR {
                    XPSNRSubMetric::Minimum
                } else {
                    XPSNRSubMetric::Weighted
                };
                if self.probing_rate > 1 {
                    let scores = if let Some(plugins) = plugins {
                        measure_xpsnr(
                            submetric,
                            chunk.proxy.as_ref().unwrap_or(&chunk.input),
                            &probe_name,
                            (chunk.start_frame as u32, chunk.end_frame as u32),
                            self.probe_res.as_ref(),
                            self.probing_rate,
                            plugins,
                        )?
                    } else {
                        bail!("XPSNR with probing_rate > 1 requires Vapoursynth to be installed");
                    };

                    aggregate_frame_scores(scores)
                } else {
                    let fl_path =
                        Path::new(&chunk.temp).join("split").join(format!("{}.json", chunk.index));

                    run_xpsnr(
                        &probe_name,
                        reference_pipe_cmd,
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
            let source_cmd = if let Some(proxy_cmd) = chunk.proxy_cmd.clone() {
                proxy_cmd
            } else {
                chunk.source_cmd.clone()
            };
            let (ff_cmd, output) = cmd.clone();

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

                let (mut source_pipe, mut enc_pipe) = {
                    if let Some(ff_cmd) = ff_cmd.as_deref() {
                        let (ffmpeg, args) = ff_cmd.split_first().expect("not empty");
                        let mut source_pipe = std::process::Command::new(ffmpeg)
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
                            })?;

                        let source_pipe_stdout = source_pipe.stdout.take().unwrap();

                        let enc_pipe = if let [cmd, args @ ..] = &*output {
                            build_encoder_pipe(cmd.clone(), args, source_pipe_stdout)?
                        } else {
                            unreachable!()
                        };
                        (Some(source_pipe), enc_pipe)
                    } else {
                        // We unfortunately have to duplicate the code like this
                        // in order to satisfy the borrow checker for `source_stdout`
                        let enc_pipe = if let [cmd, args @ ..] = &*output {
                            build_encoder_pipe(cmd.clone(), args, source_stdout)?
                        } else {
                            unreachable!()
                        };
                        (None, enc_pipe)
                    }
                };

                // Drop stdout to prevent buffer deadlock
                drop(enc_pipe.stdout.take());

                let source_stderr = source.stderr.take().unwrap();
                let stderr_thread1 = thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut stderr = source_stderr;
                    stderr.read_to_end(&mut buf).ok();
                    buf
                });

                let source_pipe_stderr = source_pipe.as_mut().map(|p| p.stderr.take().unwrap());
                let stderr_thread2 = source_pipe_stderr.map(|source_pipe_stderr| {
                    thread::spawn(move || {
                        let mut buf = Vec::new();
                        let mut stderr = source_pipe_stderr;
                        stderr.read_to_end(&mut buf).ok();
                        buf
                    })
                });

                let enc_pipe_stderr = enc_pipe.stderr.take().unwrap();
                let stderr_thread3 = thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut stderr = enc_pipe_stderr;
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

                if let Some(source_pipe) = source_pipe.as_mut() {
                    let _ = source_pipe.wait();
                };
                let _ = source.wait();

                // Collect stderr after process finishes
                let stderr_handles = (
                    stderr_thread1.join().unwrap_or_default(),
                    stderr_thread2.map(|t| t.join().unwrap_or_default()),
                    stderr_thread3.join().unwrap_or_default(),
                );

                if !enc_status.success() {
                    return Err(EncoderCrash {
                        exit_status:        enc_status,
                        source_pipe_stderr: stderr_handles.0.into(),
                        ffmpeg_pipe_stderr: stderr_handles.1.map(|h| h.into()),
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
        plugins: Option<&VapoursynthPlugins>,
    ) -> anyhow::Result<()> {
        chunk.tq_cq = Some(self.per_shot_target_quality(chunk, worker_id, plugins)?);
        Ok(())
    }
}

#[allow(clippy::result_large_err)]
fn build_encoder_pipe(
    cmd: Cow<'_, str>,
    args: &[Cow<'_, str>],
    in_pipe: impl Into<Stdio>,
) -> Result<Child, EncoderCrash> {
    std::process::Command::new(cmd.as_ref())
        .args(args.iter().map(AsRef::as_ref))
        .stdin(in_pipe)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| EncoderCrash {
            exit_status:        std::process::ExitStatus::default(),
            source_pipe_stderr: String::new().into(),
            ffmpeg_pipe_stderr: None,
            stderr:             format!("Failed to spawn encoder: {e}").into(),
            stdout:             String::new().into(),
        })
}

fn predict_quantizer(
    lower_quantizer_limit: u32,
    upper_quantizer_limit: u32,
    quantizer_score_history: &[(u32, f64)],
    target_range: (f64, f64),
    interp_method: Option<(InterpolationMethod, InterpolationMethod)>,
) -> u32 {
    let target = (target_range.0 + target_range.1) / 2.0;
    let binary_search = (lower_quantizer_limit + upper_quantizer_limit) / 2;

    let predicted_quantizer = match quantizer_score_history.len() {
        0..=1 => binary_search as f64,
        n => {
            // Sort history by quantizer
            let mut sorted = quantizer_score_history.to_vec();
            sorted.sort_by(|(_, s1), (_, s2)| {
                s1.partial_cmp(s2).unwrap_or(std::cmp::Ordering::Equal)
            });

            let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                sorted.iter().map(|(q, s)| (*s, *q as f64)).unzip();

            let result = match n {
                2 => {
                    // 3rd probe: linear interpolation
                    linear_interpolate(
                        &[scores[0], scores[1]],
                        &[quantizers[0], quantizers[1]],
                        target,
                    )
                },
                3 => {
                    // 4th probe: configurable method
                    let method =
                        interp_method.map(|(m, _)| m).unwrap_or(InterpolationMethod::Natural);
                    match method {
                        InterpolationMethod::Linear => linear_interpolate(
                            &[scores[0], scores[1]],
                            &[quantizers[0], quantizers[1]],
                            target,
                        ),
                        InterpolationMethod::Quadratic => quadratic_interpolate(
                            &[scores[0], scores[1], scores[2]],
                            &[quantizers[0], quantizers[1], quantizers[2]],
                            target,
                        ),
                        InterpolationMethod::Natural => {
                            natural_cubic_spline(&scores, &quantizers, target)
                        },
                        _ => None,
                    }
                },
                4 => {
                    // 5th probe: configurable method
                    let method =
                        interp_method.map(|(_, m)| m).unwrap_or(InterpolationMethod::Pchip);
                    let s: &[f64; 4] = &scores[..4].try_into().unwrap();
                    let q: &[f64; 4] = &quantizers[..4].try_into().unwrap();

                    match method {
                        InterpolationMethod::Linear => {
                            linear_interpolate(&[s[0], s[1]], &[q[0], q[1]], target)
                        },
                        InterpolationMethod::Quadratic => {
                            quadratic_interpolate(&[s[0], s[1], s[2]], &[q[0], q[1], q[2]], target)
                        },
                        InterpolationMethod::Natural => {
                            natural_cubic_spline(&scores, &quantizers, target)
                        },
                        InterpolationMethod::Pchip => pchip_interpolate(s, q, target),
                        InterpolationMethod::Catmull => catmull_rom_interpolate(s, q, target),
                        InterpolationMethod::Akima => akima_interpolate(s, q, target),
                        InterpolationMethod::CubicPolynomial => {
                            cubic_polynomial_interpolate(s, q, target)
                        },
                    }
                },
                _ => None,
            };

            result.unwrap_or_else(|| {
                trace!("Interpolation failed, falling back to binary search");
                binary_search as f64
            })
        },
    };

    // Round the result of the interpolation to the nearest integer
    (predicted_quantizer.round() as u32).clamp(lower_quantizer_limit, upper_quantizer_limit)
}

fn within_range(score: f64, target_range: (f64, f64)) -> bool {
    score >= target_range.0 && score <= target_range.1
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
    target: (f64, f64),
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
        "chunk {name}: Target={min}-{max}, Metric={target_metric}, P-Rate={rate}, \
         P-Speed={speed:?}, {frame_count} frames
       TQ-Probes: {history:.2?}{suffix}
       Final Q={target_quantizer:.0}, Final Score={target_score:.2}",
        name = chunk_name,
        min = target.0,
        max = target.1,
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
        let target_range = (79.5, 80.5);

        for _ in 1..=10 {
            let next_quantizer = predict_quantizer(lo, hi, &history, target_range, None);

            // Check if this quantizer was already probed
            if let Some((_quantizer, _score)) =
                history.iter().find(|(quantizer, _)| *quantizer == next_quantizer)
            {
                break;
            }

            if let Some(&score) = scores.get(&next_quantizer) {
                history.push((next_quantizer, score));

                if within_range(score, target_range) {
                    break;
                }

                if score > target_range.1 {
                    lo = (next_quantizer + 1).min(hi);
                } else if score < target_range.0 {
                    hi = (next_quantizer - 1).max(lo);
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
            assert!(within_range(result.last().unwrap().1, (79.5, 80.5)));
        }
    }
}
