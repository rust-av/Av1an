#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::cmp;
use std::fmt::Display;
use std::iter::Iterator;
use std::path::PathBuf;
use std::process::Command;

use arrayvec::ArrayVec;
use cfg_if::cfg_if;
use ffmpeg::format::Pixel;
use itertools::chain;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ffmpeg::compose_ffmpeg_pipe;
use crate::{inplace_vec, into_array, into_vec, list_index};

const NULL: &str = if cfg!(windows) { "nul" } else { "/dev/null" };

#[allow(non_camel_case_types)]
#[derive(
  Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, strum::EnumString, strum::IntoStaticStr,
)]
pub enum Encoder {
  aom,
  rav1e,
  vpx,
  #[strum(serialize = "svt-av1")]
  svt_av1,
  x264,
  x265,
}

#[tracing::instrument(level = "debug")]
pub(crate) fn parse_svt_av1_version(version: &[u8]) -> Option<(u32, u32, u32)> {
  let v_idx = memchr::memchr(b'v', version)?;
  let s = version.get(v_idx + 1..)?;
  let s = simdutf8::basic::from_utf8(s).ok()?;
  let version = s
    .split_ascii_whitespace()
    .next()?
    .split('.')
    .filter_map(|s| s.split('-').next())
    .filter_map(|s| s.parse::<u32>().ok())
    .collect::<ArrayVec<u32, 3>>();

  if let [major, minor, patch] = version[..] {
    Some((major, minor, patch))
  } else {
    None
  }
}

pub static USE_OLD_SVT_AV1: Lazy<bool> = Lazy::new(|| {
  let version = Command::new("SvtAv1EncApp")
    .arg("--version")
    .output()
    .unwrap();

  if let Some((major, minor, _)) = parse_svt_av1_version(&version.stdout) {
    match major {
      0 => minor < 9,
      1.. => false,
    }
  } else {
    // assume an old version of SVT-AV1 if the version failed to parse, as
    // the format for v0.9.0+ should be the same
    true
  }
});

impl Display for Encoder {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(<&'static str>::from(self))
  }
}

impl Encoder {
  /// Composes 1st pass command for 1 pass encoding
  pub fn compose_1_1_pass(self, params: Vec<String>, output: String) -> Vec<String> {
    match self {
      Self::aom => chain!(
        into_array!["aomenc", "--passes=1"],
        params,
        into_array!["-o", output, "-"],
      )
      .collect(),
      Self::rav1e => chain!(
        into_array!["rav1e", "-", "-y"],
        params,
        into_array!["--output", output]
      )
      .collect(),
      Self::vpx => chain!(
        into_array!["vpxenc", "--passes=1"],
        params,
        into_array!["-o", output, "-"]
      )
      .collect(),
      Self::svt_av1 => chain!(
        into_array!["SvtAv1EncApp", "-i", "stdin", "--progress", "2"],
        params,
        into_array!["-b", output],
      )
      .collect(),
      Self::x264 => chain!(
        into_array![
          "x264",
          "--stitchable",
          "--log-level",
          "error",
          "--demuxer",
          "y4m",
        ],
        params,
        into_array!["-", "-o", output]
      )
      .collect(),
      Self::x265 => chain!(
        into_array!["x265", "--y4m"],
        params,
        into_array!["--input", "-", "-o", output]
      )
      .collect(),
    }
  }

  /// Composes 1st pass command for 2 pass encoding
  pub fn compose_1_2_pass(self, params: Vec<String>, fpf: &str) -> Vec<String> {
    match self {
      Self::aom => chain!(
        into_array!["aomenc", "--passes=2", "--pass=1"],
        params,
        into_array![format!("--fpf={fpf}.log"), "-o", NULL, "-"],
      )
      .collect(),
      Self::rav1e => chain!(
        into_array!["rav1e", "-", "-y", "--quiet",],
        params,
        into_array!["--first-pass", format!("{fpf}.stat"), "--output", NULL]
      )
      .collect(),
      Self::vpx => chain!(
        into_array!["vpxenc", "--passes=2", "--pass=1"],
        params,
        into_array![format!("--fpf={fpf}.log"), "-o", NULL, "-"],
      )
      .collect(),
      Self::svt_av1 => chain!(
        into_array![
          "SvtAv1EncApp",
          "-i",
          "stdin",
          "--progress",
          "2",
          "--irefresh-type",
          "2",
        ],
        params,
        into_array!["--pass", "1", "--stats", format!("{fpf}.stat"), "-b", NULL,],
      )
      .collect(),
      Self::x264 => chain!(
        into_array![
          "x264",
          "--stitchable",
          "--log-level",
          "error",
          "--pass",
          "1",
          "--demuxer",
          "y4m",
        ],
        params,
        into_array!["--stats", format!("{fpf}.log"), "-", "-o", NULL]
      )
      .collect(),
      Self::x265 => chain!(
        into_array![
          "x265",
          "--repeat-headers",
          "--log-level",
          "error",
          "--pass",
          "1",
          "--y4m",
        ],
        params,
        into_array![
          "--stats",
          format!("{fpf}.log"),
          "--analysis-reuse-file",
          format!("{fpf}_analysis.dat"),
          "--input",
          "-",
          "-o",
          NULL
        ]
      )
      .collect(),
    }
  }

  /// Composes 2st pass command for 2 pass encoding
  pub fn compose_2_2_pass(self, params: Vec<String>, fpf: &str, output: String) -> Vec<String> {
    match self {
      Self::aom => chain!(
        into_array!["aomenc", "--passes=2", "--pass=2"],
        params,
        into_array![format!("--fpf={fpf}.log"), "-o", output, "-"],
      )
      .collect(),
      Self::rav1e => chain!(
        into_array!["rav1e", "-", "-y", "--quiet",],
        params,
        into_array!["--second-pass", format!("{fpf}.stat"), "--output", output]
      )
      .collect(),
      Self::vpx => chain!(
        into_array!["vpxenc", "--passes=2", "--pass=2"],
        params,
        into_array![format!("--fpf={fpf}.log"), "-o", output, "-"],
      )
      .collect(),
      Self::svt_av1 => chain!(
        into_array![
          "SvtAv1EncApp",
          "-i",
          "stdin",
          "--progress",
          "2",
          "--irefresh-type",
          "2",
        ],
        params,
        into_array![
          "--pass",
          "2",
          "--stats",
          format!("{fpf}.stat"),
          "-b",
          output,
        ],
      )
      .collect(),
      Self::x264 => chain!(
        into_array![
          "x264",
          "--stitchable",
          "--log-level",
          "error",
          "--pass",
          "2",
          "--demuxer",
          "y4m",
        ],
        params,
        into_array!["--stats", format!("{fpf}.log"), "-", "-o", output]
      )
      .collect(),
      Self::x265 => chain!(
        into_array![
          "x265",
          "--repeat-headers",
          "--log-level",
          "error",
          "--pass",
          "2",
          "--y4m",
        ],
        params,
        into_array![
          "--stats",
          format!("{fpf}.log"),
          "--analysis-reuse-file",
          format!("{fpf}_analysis.dat"),
          "--input",
          "-",
          "-o",
          output
        ]
      )
      .collect(),
    }
  }

  /// Returns default settings for the encoder.
  /// These will be excluded if the user specifies any custom params.
  pub fn get_default_arguments(self, (cols, rows): (u32, u32)) -> Vec<String> {
    /// Integer log base 2
    pub const fn ilog2(x: u32) -> u32 {
      // TODO: switch to built-in integer log2 functions once they are stabilized
      // https://github.com/rust-lang/rust/issues/70887
      if x == 0 {
        0
      } else {
        u32::BITS - 1 - x.leading_zeros()
      }
    }

    match self {
      // aomenc automatically infers the correct bit depth, and thus for aomenc, not specifying
      // the bit depth is actually more accurate because if for example you specify
      // `--pix-format yuv420p`, aomenc will encode 10-bit when that is not actually the desired
      // pixel format.
      Encoder::aom => {
        let defaults: Vec<String> = into_vec![
          "--threads=8",
          "--cpu-used=6",
          "--end-usage=q",
          "--cq-level=30",
        ];

        if cols > 1 || rows > 1 {
          let columns = ilog2(cols);
          let rows = ilog2(rows);

          let aom_tiles: Vec<String> = into_vec![
            format!("--tile-columns={columns}"),
            format!("--tile-rows={rows}")
          ];
          chain!(defaults, aom_tiles).collect()
        } else {
          defaults
        }
      }
      Encoder::rav1e => {
        let defaults: Vec<String> = into_vec!["--speed", "6", "--quantizer", "100"];

        if cols > 1 || rows > 1 {
          let tiles: Vec<String> = into_vec!["--tiles", format!("{}", cols * rows)];
          chain!(defaults, tiles).collect()
        } else {
          defaults
        }
      }
      // vpxenc does not infer the pixel format from the input, so `-b 10` is still required
      // to work with the default pixel format (yuv420p10le).
      Encoder::vpx => {
        let defaults = into_vec![
          "--codec=vp9",
          "-b",
          "10",
          "--profile=2",
          "--threads=4",
          "--cpu-used=2",
          "--end-usage=q",
          "--cq-level=30",
          "--row-mt=1",
          "--auto-alt-ref=6",
        ];

        if cols > 1 || rows > 1 {
          let columns = ilog2(cols);
          let rows = ilog2(rows);

          let aom_tiles: Vec<String> = into_vec![
            format!("--tile-columns={columns}"),
            format!("--tile-rows={rows}")
          ];
          chain!(defaults, aom_tiles).collect()
        } else {
          defaults
        }
      }
      Encoder::svt_av1 => {
        let defaults = into_vec!["--preset", "4", "--rc", "0", "--crf", "25"];
        if cols > 1 || rows > 1 {
          let columns = ilog2(cols);
          let rows = ilog2(rows);

          let tiles: Vec<String> = into_vec![
            "--tile-columns",
            columns.to_string(),
            "--tile-rows",
            rows.to_string()
          ];
          chain!(defaults, tiles).collect()
        } else {
          defaults
        }
      }
      Encoder::x264 => into_vec!["--preset", "slow", "--crf", "25"],
      Encoder::x265 => into_vec![
        "--preset",
        "slow",
        "--crf",
        "25",
        "-D",
        "10",
        "--level-idc",
        "5.0",
      ],
    }
  }

  /// Returns additional settings for the encoder that are strongly recommended
  /// for use with av1an. Can still be overridden if the user specifies the same setting.
  pub fn get_additional_arguments(self) -> Vec<String> {
    match self {
      Encoder::aom => {
        into_vec!["--disable-kf", "--kf-max-dist=9999"]
      }
      Encoder::rav1e => {
        into_vec!["--keyint", "0", "--no-scene-detection"]
      }
      Encoder::vpx => {
        into_vec!["--disable-kf", "--kf-max-dist=9999"]
      }
      Encoder::svt_av1 => {
        into_vec!["--keyint", "0", "--scd", "0"]
      }
      Encoder::x264 => into_vec!["--keyint", "infinite", "--scenecut", "0"],
      Encoder::x265 => into_vec!["--keyint", "-1", "--scenecut", "0"],
    }
  }

  /// Return number of default passes for encoder
  pub const fn get_default_pass(self) -> u8 {
    match self {
      Self::aom | Self::vpx => 2,
      _ => 1,
    }
  }

  /// Default quantizer range target quality mode
  pub const fn get_default_cq_range(self) -> (usize, usize) {
    match self {
      Self::aom | Self::vpx => (15, 55),
      Self::rav1e => (50, 140),
      Self::svt_av1 => (15, 50),
      Self::x264 | Self::x265 => (15, 35),
    }
  }

  /// Returns help command for encoder
  pub const fn help_command(self) -> [&'static str; 2] {
    match self {
      Self::aom => ["aomenc", "--help"],
      Self::rav1e => ["rav1e", "--help"],
      Self::vpx => ["vpxenc", "--help"],
      Self::svt_av1 => ["SvtAv1EncApp", "--help"],
      Self::x264 => ["x264", "--fullhelp"],
      Self::x265 => ["x265", "--fullhelp"],
    }
  }

  /// Returns version text for encoder, or None if encoder is not available in PATH
  pub fn version_text(self) -> Option<String> {
    match self {
      Self::aom => {
        let result = Command::new("aomenc").arg("--help").output().ok()?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        let version_line = stdout.lines().find(|line| line.starts_with("    av1"))?;
        Some(
          version_line
            .split_once('-')
            .unwrap()
            .1
            .replace("(default)", "")
            .trim()
            .to_string(),
        )
      }
      Self::rav1e => {
        let result = Command::new("rav1e").arg("--version").output().ok()?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        let version_line = stdout.lines().find(|line| line.starts_with("rav1e"))?;
        Some(version_line.to_string())
      }
      Self::vpx => {
        let result = Command::new("vpxenc").arg("--help").output().ok()?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        let version_line = stdout.lines().find(|line| line.starts_with("    vp9"))?;
        Some(
          version_line
            .split_once('-')
            .unwrap()
            .1
            .replace("(default)", "")
            .trim()
            .to_string(),
        )
      }
      Self::svt_av1 => {
        let result = Command::new("SvtAv1EncApp")
          .arg("--version")
          .output()
          .ok()?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        let version_line = stdout.lines().find(|line| line.starts_with("SVT-AV1"))?;
        Some(version_line.to_string())
      }
      Self::x264 => {
        let result = Command::new("x264").arg("--version").output().ok()?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        let version_line = stdout.lines().find(|line| line.starts_with("x264"))?;
        Some(version_line.to_string())
      }
      Self::x265 => {
        let result = Command::new("x265").arg("--version").output().ok()?;
        let stderr = String::from_utf8_lossy(&result.stderr);
        let version_line = stderr.lines().find(|line| line.starts_with("x265"))?;
        Some(version_line.split_once(':').unwrap().1.trim().to_string())
      }
    }
  }

  /// Get the name of the executable/binary for the encoder
  pub const fn bin(self) -> &'static str {
    match self {
      Self::aom => "aomenc",
      Self::rav1e => "rav1e",
      Self::vpx => "vpxenc",
      Self::svt_av1 => "SvtAv1EncApp",
      Self::x264 => "x264",
      Self::x265 => "x265",
    }
  }

  /// Get the name of the video format associated with the encoder
  pub const fn format(self) -> &'static str {
    match self {
      Self::aom | Self::rav1e | Self::svt_av1 => "av1",
      Self::vpx => "vpx",
      Self::x264 => "h264",
      Self::x265 => "h265",
    }
  }

  /// Get the default output extension for the encoder
  pub const fn output_extension(&self) -> &'static str {
    match &self {
      Self::aom | Self::rav1e | Self::vpx | Self::svt_av1 => "ivf",
      Self::x264 | Self::x265 => "mkv",
    }
  }

  /// Returns function pointer used for matching Q/CRF arguments in command line
  fn q_match_fn(self) -> fn(&str) -> bool {
    match self {
      Self::aom | Self::vpx => |p| p.starts_with("--cq-level="),
      Self::rav1e => |p| p == "--quantizer",
      Self::svt_av1 => |p| matches!(p, "--qp" | "-q" | "--crf"),
      Self::x264 | Self::x265 => |p| p == "--crf",
    }
  }

  fn replace_q(self, index: usize, q: usize) -> (usize, String) {
    match self {
      Self::aom | Self::vpx => (index, format!("--cq-level={q}")),
      Self::rav1e | Self::svt_av1 | Self::x265 | Self::x264 => (index + 1, q.to_string()),
    }
  }

  fn insert_q(self, q: usize) -> ArrayVec<String, 2> {
    let mut output = ArrayVec::new();
    match self {
      Self::aom | Self::vpx => {
        output.push(format!("--cq-level={q}"));
      }
      Self::rav1e => {
        output.push("--quantizer".into());
        output.push(q.to_string());
      }
      Self::svt_av1 | Self::x264 | Self::x265 => {
        output.push("--crf".into());
        output.push(q.to_string());
      }
    }
    output
  }

  /// Returns changed q/crf in command line arguments
  pub fn man_command(self, mut params: Vec<String>, q: usize) -> Vec<String> {
    let index = list_index(&params, self.q_match_fn());
    if let Some(index) = index {
      let (replace_index, replace_q) = self.replace_q(index, q);
      params[replace_index] = replace_q;
    } else {
      let args = self.insert_q(q);
      params.extend_from_slice(&args);
    }

    params
  }

  /// Parses the number of encoded frames
  pub(crate) fn parse_encoded_frames(self, line: &str) -> Option<u64> {
    use crate::parse::*;

    match self {
      Self::aom | Self::vpx => {
        cfg_if! {
          if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
            if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
              return unsafe { parse_aom_vpx_frames_sse41(line.as_bytes()) };
            }
          }
        }

        parse_aom_vpx_frames(line)
      }
      Self::rav1e => parse_rav1e_frames(line),
      Self::svt_av1 => parse_svt_av1_frames(line),
      Self::x264 | Self::x265 => parse_x26x_frames(line),
    }
  }

  /// Returns command used for target quality probing
  pub fn construct_target_quality_command(
    self,
    threads: usize,
    q: usize,
  ) -> Vec<Cow<'static, str>> {
    match &self {
      Self::aom => inplace_vec![
        "aomenc",
        "--passes=1",
        format!("--threads={threads}"),
        "--tile-columns=2",
        "--tile-rows=1",
        "--end-usage=q",
        "-b",
        "8",
        "--cpu-used=6",
        format!("--cq-level={q}"),
        "--enable-filter-intra=0",
        "--enable-smooth-intra=0",
        "--enable-paeth-intra=0",
        "--enable-cfl-intra=0",
        "--enable-angle-delta=0",
        "--reduced-tx-type-set=1",
        "--enable-intra-edge-filter=0",
        "--enable-order-hint=0",
        "--enable-flip-idtx=0",
        "--enable-global-motion=0",
        "--enable-cdef=0",
        "--max-reference-frames=3",
        "--cdf-update-mode=2",
        "--enable-tpl-model=0",
        "--sb-size=64",
        "--min-partition-size=32",
        "--disable-kf",
        "--kf-max-dist=9999"
      ],
      Self::rav1e => inplace_vec![
        "rav1e",
        "-y",
        "-s",
        "10",
        "--threads",
        threads.to_string(),
        "--tiles",
        "16",
        "--quantizer",
        q.to_string(),
        "--low-latency",
        "--rdo-lookahead-frames",
        "5",
        "--no-scene-detection",
      ],
      Self::vpx => inplace_vec![
        "vpxenc",
        "-b",
        "10",
        "--profile=2",
        "--passes=1",
        "--pass=1",
        "--codec=vp9",
        format!("--threads={threads}"),
        "--cpu-used=9",
        "--end-usage=q",
        format!("--cq-level={q}"),
        "--row-mt=1",
        "--disable-kf",
        "--kf-max-dist=9999"
      ],
      Self::svt_av1 => {
        if *USE_OLD_SVT_AV1 {
          inplace_vec![
            "SvtAv1EncApp",
            "-i",
            "stdin",
            "--lp",
            threads.to_string(),
            "--preset",
            "8",
            "--keyint",
            "240",
            "--crf",
            q.to_string(),
            "--tile-rows",
            "1",
            "--tile-columns",
            "2",
            "--pred-struct",
            "0",
            "--sg-filter-mode",
            "0",
            "--enable-restoration-filtering",
            "0",
            "--cdef-level",
            "0",
            "--disable-dlf",
            "0",
            "--mrp-level",
            "0",
            "--enable-mfmv",
            "0",
            "--enable-local-warp",
            "0",
            "--enable-global-motion",
            "0",
            "--enable-interintra-comp",
            "0",
            "--obmc-level",
            "0",
            "--rdoq-level",
            "0",
            "--filter-intra-level",
            "0",
            "--enable-intra-edge-filter",
            "0",
            "--enable-pic-based-rate-est",
            "0",
            "--pred-me",
            "0",
            "--bipred-3x3",
            "0",
            "--compound",
            "0",
            "--ext-block",
            "0",
            "--hbd-md",
            "0",
            "--palette-level",
            "0",
            "--umv",
            "0",
            "--tf-level",
            "3",
          ]
        } else {
          inplace_vec![
            "SvtAv1EncApp",
            "-i",
            "stdin",
            "--lp",
            threads.to_string(),
            "--preset",
            "12",
            "--keyint",
            "240",
            "--crf",
            q.to_string(),
            "--tile-rows",
            "1",
            "--tile-columns",
            "2",
          ]
        }
      }
      Self::x264 => inplace_vec![
        "x264",
        "--log-level",
        "error",
        "--demuxer",
        "y4m",
        "-",
        "--no-progress",
        "--threads",
        threads.to_string(),
        "--preset",
        "medium",
        "--crf",
        q.to_string(),
      ],
      Self::x265 => inplace_vec![
        "x265",
        "--log-level",
        "0",
        "--no-progress",
        "--y4m",
        "--frame-threads",
        cmp::min(threads, 16).to_string(),
        "--preset",
        "fast",
        "--crf",
        q.to_string(),
        "--input",
        "-",
      ],
    }
  }

  /// Returns command used for target quality probing (slow, correctness focused version)
  pub fn construct_target_quality_command_probe_slow(self, q: usize) -> Vec<Cow<'static, str>> {
    match &self {
      Self::aom => inplace_vec!["aomenc", "--passes=1", format!("--cq-level={q}")],
      Self::rav1e => inplace_vec!["rav1e", "-y", "--quantizer", q.to_string()],
      Self::vpx => inplace_vec![
        "vpxenc",
        "--passes=1",
        "--pass=1",
        "--codec=vp9",
        "--end-usage=q",
        format!("--cq-level={q}"),
      ],
      Self::svt_av1 => inplace_vec!["SvtAv1EncApp", "-i", "stdin", "--crf", q.to_string()],
      Self::x264 => inplace_vec![
        "x264",
        "--log-level",
        "error",
        "--demuxer",
        "y4m",
        "-",
        "--no-progress",
        "--crf",
        q.to_string(),
      ],
      Self::x265 => inplace_vec![
        "x265",
        "--log-level",
        "0",
        "--no-progress",
        "--y4m",
        "--crf",
        q.to_string(),
        "--input",
        "-",
      ],
    }
  }

  /// Function `remove_patterns` that takes in args and patterns and removes all instances of the patterns from the args.
  pub fn remove_patterns(args: &mut Vec<String>, patterns: &[&str]) {
    for pattern in patterns {
      if let Some(index) = args.iter().position(|value| value.contains(pattern)) {
        args.remove(index);
        // If pattern does not contain =, we need to remove the index that follows.
        if !pattern.contains('=') {
          args.remove(index);
        }
      }
    }
  }

  #[allow(clippy::too_many_arguments)]
  /// Constructs tuple of commands for target quality probing
  pub fn probe_cmd(
    self,
    temp: String,
    chunk_index: usize,
    q: usize,
    pix_fmt: Pixel,
    probing_rate: usize,
    vmaf_threads: usize,
    mut video_params: Vec<String>,
    probe_slow: bool,
  ) -> (Vec<String>, Vec<Cow<'static, str>>) {
    let pipe = compose_ffmpeg_pipe(
      [
        "-vf",
        format!("select=not(mod(n\\,{probing_rate}))").as_str(),
        "-vsync",
        "0",
      ],
      pix_fmt,
    );

    let probe_name = format!("v_{q}_{chunk_index}.ivf");
    let mut probe = PathBuf::from(temp);
    probe.push("split");
    probe.push(&probe_name);
    let probe_path = probe.to_str().unwrap().to_owned();

    let params: Vec<Cow<str>> = if probe_slow {
      let patterns = [
        "--cq-level=",
        "--passes=",
        "--pass=",
        "--crf",
        "--quantizer",
      ];
      Self::remove_patterns(&mut video_params, &patterns);
      let mut ps = self.construct_target_quality_command_probe_slow(q);

      ps.reserve(video_params.len());
      for arg in video_params {
        ps.push(Cow::Owned(arg));
      }

      ps
    } else {
      self.construct_target_quality_command(vmaf_threads, q)
    };

    let output: Vec<Cow<str>> = match self {
      Self::svt_av1 => chain!(params, into_array!["-b", probe_path]).collect(),
      Self::aom | Self::rav1e | Self::vpx | Self::x264 => {
        chain!(params, into_array!["-o", probe_path, "-"]).collect()
      }
      Self::x265 => chain!(params, into_array!["-o", probe_path]).collect(),
    };

    (pipe, output)
  }

  pub fn get_format_bit_depth(self, format: Pixel) -> Result<usize, UnsupportedPixelFormatError> {
    macro_rules! impl_this_function {
      ($($encoder:ident),*) => {
        match self {
          $(
            Encoder::$encoder => paste::paste! { [<get_ $encoder _format_bit_depth>](format) },
          )*
        }
      };
    }
    impl_this_function!(x264, x265, vpx, aom, rav1e, svt_av1)
  }
}

#[derive(Error, Debug)]
pub enum UnsupportedPixelFormatError {
  #[error("{0} does not support {1:?}")]
  UnsupportedFormat(Encoder, Pixel),
}

macro_rules! create_get_format_bit_depth_function {
  ($encoder:ident, 8: $_8bit_fmts:expr, 10: $_10bit_fmts:expr, 12: $_12bit_fmts:expr) => {
    paste::paste! {
      pub fn [<get_ $encoder _format_bit_depth>](format: Pixel) -> Result<usize, UnsupportedPixelFormatError> {
        use Pixel::*;
        if $_8bit_fmts.contains(&format) {
          Ok(8)
        } else if $_10bit_fmts.contains(&format) {
          Ok(10)
        } else if $_12bit_fmts.contains(&format) {
          Ok(12)
        } else {
          Err(UnsupportedPixelFormatError::UnsupportedFormat(Encoder::$encoder, format))
        }
      }
    }
  };
}

// The supported bit depths are taken from ffmpeg,
// e.g.: `ffmpeg -h encoder=libx264`
create_get_format_bit_depth_function!(
  x264,
   8: [YUV420P, YUVJ420P, YUV422P, YUVJ422P, YUV444P, YUVJ444P, NV12, NV16, NV21, GRAY8],
  10: [YUV420P10LE, YUV422P10LE, YUV444P10LE, NV20LE, GRAY10LE],
  12: []
);
create_get_format_bit_depth_function!(
  x265,
   8: [YUV420P, YUVJ420P, YUV422P, YUVJ422P, YUV444P, YUVJ444P, GBRP, GRAY8],
  10: [YUV420P10LE, YUV422P10LE, YUV444P10LE, GBRP10LE, GRAY10LE],
  12: [YUV420P12LE, YUV422P12LE, YUV444P12LE, GBRP12LE, GRAY12LE]
);
create_get_format_bit_depth_function!(
  vpx,
   8: [YUV420P, YUVA420P, YUV422P, YUV440P, YUV444P, GBRP],
  10: [YUV420P10LE, YUV422P10LE, YUV440P10LE, YUV444P10LE, GBRP10LE],
  12: [YUV420P12LE, YUV422P12LE, YUV440P12LE, YUV444P12LE, GBRP12LE]
);
create_get_format_bit_depth_function!(
  aom,
   8: [YUV420P, YUV422P, YUV444P, GBRP, GRAY8],
  10: [YUV420P10LE, YUV422P10LE, YUV444P10LE, GBRP10LE, GRAY10LE],
  12: [YUV420P12LE, YUV422P12LE, YUV444P12LE, GBRP12LE, GRAY12LE,]
);
create_get_format_bit_depth_function!(
  rav1e,
   8: [YUV420P, YUVJ420P, YUV422P, YUVJ422P, YUV444P, YUVJ444P],
  10: [YUV420P10LE, YUV422P10LE, YUV444P10LE],
  12: [YUV420P12LE, YUV422P12LE, YUV444P12LE,]
);
create_get_format_bit_depth_function!(
  svt_av1,
   8: [YUV420P],
  10: [YUV420P10LE],
  12: []
);
