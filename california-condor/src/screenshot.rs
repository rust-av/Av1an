//! Generates the documentation screenshots for the TUI.
//!
//! Each screen is rendered offscreen through a ratatui [`TestBackend`] — the
//! exact same `TuiApp::render` code the live interface uses — so the images can
//! never drift from the real interface: any change to a screen is reflected the
//! next time the harness runs. The resulting character grid is rasterized with
//! a monospace font into a 1920x1080 image and encoded as AVIF, in both the
//! light and dark [`Theme`] variants. Documentation picks the right one with a
//! `prefers-color-scheme` media query.
//!
//! This whole module is compiled only under the `screenshots` feature so the
//! shipping binary does not link `ab_glyph`. AVIF encoding is done by FFmpeg.
//! See `src/bin/gen_screenshots.rs` to produce the files and
//! `tests/screenshots.rs` for the in-memory smoke test.

use std::{
    collections::BTreeMap,
    env,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use andean_condor::{
    core::input::{
        clip_info::{ClipInfo, TransferCharacteristic},
        pixel_format::PixelFormat,
    },
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{Encoder, EncoderBase},
        scene::Scene,
        sequence::{scene_concatenator::ConcatMethod, target_quality::types::ProbeStatistic},
    },
};
use clap::CommandFactory;
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    apps::{
        TuiApp,
        benchmarker::{BenchmarkerApp, BenchmarkerState, WorkerStatus},
        noise_detection::{NoiseDetectionApp, NoiseDetectionState},
        parallel_encoder::{ParallelEncoderApp, ParallelEncoderState, SceneEncoder},
        scene_concatenator::{SceneConcatenatorApp, SceneConcatenatorState},
        scene_detection::{SceneDetectionApp, SceneDetectionState},
        target_quality::{QualityPass, TargetQualityApp, TargetQualityState},
    },
    commands::{CondorCli, version::render_version},
    configuration::CliSequenceData,
    test_helpers::TestVideo,
    theme::{Theme, ThemeMode},
};

/// Terminal size, in cells, shared by every screenshot.
///
/// 54 rows rather than the tighter 45: the `--version --verbose` report grows
/// with the machine's installed plugins, encoders and compute devices, and must
/// fit whole rather than being cut off at the bottom.
const COLS: u16 = 160;
const ROWS: u16 = 54;
/// Pixel size of one cell; 160 * 12 = 1920 and 54 * 20 = 1080.
const CELL_W: u32 = 12;
const CELL_H: u32 = 20;
/// Font em size, in pixels, for the glyph rasterizer.
const FONT_PX: f32 = 17.0;

const WIDTH: u32 = COLS as u32 * CELL_W;
const HEIGHT: u32 = ROWS as u32 * CELL_H;

// ---------------------------------------------------------------------------
// Font
// ---------------------------------------------------------------------------

/// Locate a monospace font. Order: `CONDOR_SCREENSHOT_FONT`, then a few
/// well-known system faces so the generator works out of the box on Windows,
/// macOS and Linux/CI. Committing a font and pointing the env var at it yields
/// identical output everywhere.
fn load_font() -> Vec<u8> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = env::var("CONDOR_SCREENSHOT_FONT") {
        candidates.push(p.into());
    }
    candidates.extend(
        [
            // Windows
            r"C:\Windows\Fonts\consola.ttf",
            r"C:\Windows\Fonts\CascadiaMono.ttf",
            r"C:\Windows\Fonts\lucon.ttf",
            // Linux (Debian/Ubuntu CI)
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            // macOS
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Monaco.ttf",
        ]
        .iter()
        .map(PathBuf::from),
    );
    for path in candidates {
        if let Ok(bytes) = fs::read(&path) {
            return bytes;
        }
    }
    panic!(
        "No monospace font found for screenshot generation. Set CONDOR_SCREENSHOT_FONT to a \
         .ttf/.otf path."
    );
}

// ---------------------------------------------------------------------------
// Rasterizer
// ---------------------------------------------------------------------------

/// Map a ratatui [`Color`](ratatui::style::Color) to RGB, resolving `Reset`
/// against the theme.
fn rgb(color: ratatui::style::Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    use ratatui::style::Color::{
        Black,
        Blue,
        Cyan,
        DarkGray,
        Gray,
        Green,
        Indexed,
        LightBlue,
        LightCyan,
        LightGreen,
        LightMagenta,
        LightRed,
        LightYellow,
        Magenta,
        Red,
        Reset,
        Rgb,
        White,
        Yellow,
    };
    match color {
        Reset => fallback,
        Black => (0, 0, 0),
        Red => (205, 0, 0),
        Green => (0, 205, 0),
        Yellow => (205, 205, 0),
        Blue => (0, 0, 238),
        Magenta => (205, 0, 205),
        Cyan => (0, 205, 205),
        Gray => (187, 187, 187),
        DarkGray => (128, 128, 128),
        LightRed => (255, 0, 0),
        LightGreen => (0, 255, 0),
        LightYellow => (255, 255, 0),
        LightBlue => (92, 92, 255),
        LightMagenta => (255, 0, 255),
        LightCyan => (0, 255, 255),
        White => (255, 255, 255),
        Rgb(r, g, b) => (r, g, b),
        Indexed(i) => xterm256(i),
    }
}

/// Rough xterm-256 to RGB for the indexed colors ratatui may emit.
fn xterm256(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => [
            (0, 0, 0),
            (205, 0, 0),
            (0, 205, 0),
            (205, 205, 0),
            (0, 0, 238),
            (205, 0, 205),
            (0, 205, 205),
            (192, 192, 192),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (92, 92, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ][i as usize],
        16..=231 => {
            let i = i - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            (
                steps[(i / 36) as usize],
                steps[((i / 6) % 6) as usize],
                steps[(i % 6) as usize],
            )
        },
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        },
    }
}

/// A minimal RGBA8 raster image, so generating screenshots needs no image
/// library. AVIF encoding is delegated to FFmpeg (see [`encode_avif`]).
struct Canvas {
    width:  u32,
    height: u32,
    data:   Vec<u8>,
}

impl Canvas {
    fn from_pixel(width: u32, height: u32, color: [u8; 4]) -> Self {
        let pixels = width as usize * height as usize;
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            data.extend_from_slice(&color);
        }
        Self {
            width,
            height,
            data,
        }
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn as_raw(&self) -> &[u8] {
        &self.data
    }

    fn get_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    fn put_pixel(&mut self, x: u32, y: u32, color: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        self.data[i..i + 4].copy_from_slice(&color);
    }
}

fn fill(img: &mut Canvas, x0: u32, y0: u32, w: u32, h: u32, color: (u8, u8, u8)) {
    let (r, g, b) = color;
    for y in y0..(y0 + h).min(img.height()) {
        for x in x0..(x0 + w).min(img.width()) {
            img.put_pixel(x, y, [r, g, b, 255]);
        }
    }
}

/// Draw box-drawing and block-element glyphs as rectangles that span the full
/// cell, the way a terminal does.
///
/// Fonts draw these glyphs at their own ink size, so when the cell is wider or
/// taller than the glyph advance — as it is here, where cells are sized for a
/// 160x54 grid rather than the font's natural metrics — borders render as
/// dashes with gaps and a gauge run renders as stripes. Terminals stretch the
/// glyphs to the cell instead, which is what makes lines and bars read as
/// contiguous. Returns `false` for anything this doesn't handle, so the caller
/// falls back to font rasterization.
fn draw_tiling_char(img: &mut Canvas, ch: char, x0: u32, y0: u32, fg: (u8, u8, u8)) -> bool {
    let x1 = x0 + CELL_W;
    let y1 = y0 + CELL_H;
    let cx = x0 + CELL_W / 2;
    let cy = y0 + CELL_H / 2;

    /// Fill a horizontal segment centred on `ymid`, spanning `xa..xb`.
    fn hline(img: &mut Canvas, xa: u32, xb: u32, ymid: u32, t: u32, c: (u8, u8, u8)) {
        let top = ymid.saturating_sub(t / 2);
        fill(img, xa, top, xb - xa, t, c);
    }
    /// Fill a vertical segment centred on `xmid`, spanning `ya..yb`.
    fn vline(img: &mut Canvas, ya: u32, yb: u32, xmid: u32, t: u32, c: (u8, u8, u8)) {
        let left = xmid.saturating_sub(t / 2);
        fill(img, left, ya, t, yb - ya, c);
    }

    // Line sets: normal `─│┌┐…`, rounded `╭╮╰╯`, thick `━┃┏┓…` and double
    // `═║╔╗…` (rendered as two hairlines, which is how a terminal draws it).
    // Each arm runs to the cell edge so neighbours join without a seam.
    let (mut h_full, mut v_full, mut h_left, mut h_right, mut v_top, mut v_bottom) =
        (false, false, false, false, false, false);
    let (thickness, double) = match ch {
        '─' | '│' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼' | '╭' | '╮' | '╰' | '╯' => {
            (2, false)
        },
        '━' | '┃' | '┏' | '┓' | '┗' | '┛' | '┣' | '┫' | '┳' | '┻' | '╋' => {
            (3, false)
        },
        '═' | '║' | '╔' | '╗' | '╚' | '╝' | '╠' | '╣' | '╦' | '╩' | '╬' => {
            (1, true)
        },
        _ => (0, false),
    };
    if thickness > 0 {
        // Which arms this character carries.
        (h_full, v_full, h_left, h_right, v_top, v_bottom) = match ch {
            '─' | '━' | '═' => (true, false, false, false, false, false),
            '│' | '┃' | '║' => (false, true, false, false, false, false),
            '┌' | '╭' | '┏' | '╔' => (false, false, false, true, false, true),
            '┐' | '╮' | '┓' | '╗' => (false, false, true, false, false, true),
            '└' | '╰' | '┗' | '╚' => (false, false, false, true, true, false),
            '┘' | '╯' | '┛' | '╝' => (false, false, true, false, true, false),
            '├' | '┣' | '╠' => (false, true, false, true, false, false),
            '┤' | '┫' | '╣' => (false, true, true, false, false, false),
            '┬' | '┳' | '╦' => (true, false, false, false, false, true),
            '┴' | '┻' | '╩' => (true, false, false, false, true, false),
            '┼' | '╋' | '╬' => (true, true, false, false, false, false),
            _ => (false, false, false, false, false, false),
        };
        // A double line is two hairlines straddling the centre, with the
        // gap between them — how a terminal draws it.
        let bands: Box<dyn Fn(&mut Canvas, u32, u32, u32, (u8, u8, u8))> = if double {
            Box::new(move |img: &mut Canvas, a: u32, b: u32, mid: u32, c| {
                hline(img, a, b, mid.saturating_sub(2), 1, c);
                hline(img, a, b, mid + 1, 1, c);
            })
        } else {
            Box::new(move |img: &mut Canvas, a: u32, b: u32, mid: u32, c| {
                hline(img, a, b, mid, thickness, c);
            })
        };
        if h_full {
            bands(img, x0, x1, cy, fg);
        }
        if h_left {
            bands(img, x0, cx, cy, fg);
        }
        if h_right {
            bands(img, cx, x1, cy, fg);
        }
        let vbands: Box<dyn Fn(&mut Canvas, u32, u32, u32, (u8, u8, u8))> = if double {
            Box::new(move |img: &mut Canvas, a: u32, b: u32, mid: u32, c| {
                vline(img, a, b, mid.saturating_sub(2), 1, c);
                vline(img, a, b, mid + 1, 1, c);
            })
        } else {
            Box::new(move |img: &mut Canvas, a: u32, b: u32, mid: u32, c| {
                vline(img, a, b, mid, thickness, c);
            })
        };
        if v_full {
            vbands(img, y0, y1, cx, fg);
        }
        if v_top {
            vbands(img, y0, cy, cx, fg);
        }
        if v_bottom {
            vbands(img, cy, y1, cx, fg);
        }
        return true;
    }

    // Checkmark and cross (✓ U+2713, ✕ U+2715). Monospace text fonts often
    // omit these, so a terminal falls back to a symbol font. We have no
    // fallback here, so draw them as vector strokes the way the boxes above
    // are drawn — guaranteed to render on every platform.
    if ch == '\u{2713}' || ch == '\u{2715}' {
        /// Draw a thick line between two points expressed in cell fractions.
        fn stroke(
            img: &mut Canvas,
            x0: u32,
            y0: u32,
            fg: (u8, u8, u8),
            from: (f64, f64),
            to: (f64, f64),
        ) {
            let ax = from.0.mul_add(CELL_W as f64, x0 as f64);
            let ay = from.1.mul_add(CELL_H as f64, y0 as f64);
            let bx = to.0.mul_add(CELL_W as f64, x0 as f64);
            let by = to.1.mul_add(CELL_H as f64, y0 as f64);
            let steps = ((bx - ax).abs().max((by - ay).abs()).ceil() as u32).max(1);
            for s in 0..=steps {
                let f = s as f64 / steps as f64;
                let px = f.mul_add(bx - ax, ax);
                let py = f.mul_add(by - ay, ay);
                fill(
                    img,
                    (px - 1.0).round().max(0.0) as u32,
                    (py - 1.0).round().max(0.0) as u32,
                    3,
                    3,
                    fg,
                );
            }
        }
        if ch == '\u{2713}' {
            // ✓: short stroke down to the vertex, long stroke up to the right.
            stroke(img, x0, y0, fg, (0.20, 0.50), (0.40, 0.74));
            stroke(img, x0, y0, fg, (0.40, 0.74), (0.82, 0.26));
        } else {
            // ✕: two diagonals forming an X.
            stroke(img, x0, y0, fg, (0.26, 0.28), (0.74, 0.72));
            stroke(img, x0, y0, fg, (0.74, 0.28), (0.26, 0.72));
        }
        return true;
    }

    // Block elements, as emitted by Gauge: full block, eighths (partial fill
    // at the gauge's leading edge), and half/vertical blocks.
    match ch {
        '\u{2588}' => fill(img, x0, y0, CELL_W, CELL_H, fg), // █ full
        '\u{2589}'..='\u{258F}' => {
            // ▉▊▋▌▍▎▏ left-aligned eighths.
            let eighths = 0x2590 - ch as u32;
            let w = (CELL_W * eighths).div_ceil(8).max(1);
            fill(img, x0, y0, w, CELL_H, fg);
        },
        '\u{2580}' => fill(img, x0, y0, CELL_W, CELL_H / 2, fg), // ▀ upper half
        '\u{2584}' => fill(img, x0, y0 + CELL_H / 2, CELL_W, CELL_H - CELL_H / 2, fg),
        // ▁▂▃▄▅▆▇ bottom eighths (LineGauge-style fills).
        '\u{2581}'..='\u{2587}' => {
            let eighths = ch as u32 - 0x2580;
            let h = (CELL_H * eighths).div_ceil(8).max(1);
            fill(img, x0, y1 - h, CELL_W, h, fg);
        },
        _ => return false,
    }
    true
}

/// Rasterize a rendered character grid into a 1920x1080 RGBA image.
fn rasterize(buf: &Buffer, theme: Theme, font: &FontVec) -> Canvas {
    let bg_default = rgb(theme.background, (0, 0, 0));
    let fg_default = rgb(theme.foreground, (255, 255, 255));
    let mut img = Canvas::from_pixel(WIDTH, HEIGHT, [
        bg_default.0,
        bg_default.1,
        bg_default.2,
        255,
    ]);

    let scale = PxScale::from(FONT_PX);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();

    let width = buf.area.width as usize;
    for (i, cell) in buf.content().iter().enumerate() {
        let cx = (i % width) as u32;
        let cy = (i / width) as u32;
        let x0 = cx * CELL_W;
        let y0 = cy * CELL_H;

        let bg = rgb(cell.bg, bg_default);
        fill(&mut img, x0, y0, CELL_W, CELL_H, bg);

        let symbol = cell.symbol();
        let Some(ch) = symbol.chars().next() else {
            continue;
        };
        if ch == ' ' || ch.is_control() {
            continue;
        }
        let fg = rgb(cell.fg, fg_default);
        // Borders and gauge runs tile seamlessly, as in a real terminal.
        if draw_tiling_char(&mut img, ch, x0, y0, fg) {
            continue;
        }
        let glyph = font
            .glyph_id(ch)
            .with_scale_and_position(scale, point(x0 as f32, y0 as f32 + ascent));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|dx, dy, coverage| {
                let px = bounds.min.x as u32 + dx;
                let py = bounds.min.y as u32 + dy;
                if px < img.width() && py < img.height() {
                    let base = img.get_pixel(px, py);
                    let a = coverage.clamp(0.0, 1.0);
                    let out = [
                        (fg.0 as f32).mul_add(a, base[0] as f32 * (1.0 - a)).round() as u8,
                        (fg.1 as f32).mul_add(a, base[1] as f32 * (1.0 - a)).round() as u8,
                        (fg.2 as f32).mul_add(a, base[2] as f32 * (1.0 - a)).round() as u8,
                        255,
                    ];
                    img.put_pixel(px, py, out);
                }
            });
        }
    }
    img
}

/// Render a frame into an offscreen buffer via the test backend.
///
/// One row shorter than the canvas: the top row is reserved for the PowerShell
/// prompt line added by [`session_buf`].
fn capture<F: FnOnce(&mut ratatui::Frame)>(render: F) -> Buffer {
    let backend = TestBackend::new(COLS, ROWS - 1);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(render).expect("draw");
    terminal.backend().buffer().clone()
}

/// Encode an RGBA canvas as AVIF using FFmpeg.
///
/// Condor already depends on FFmpeg and CI provides it, so the screenshot
/// tooling pipes raw RGBA to FFmpeg instead of linking an AVIF encoder.
fn encode_avif(img: &Canvas, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let ffmpeg = env::var("CONDOR_SCREENSHOT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
    let size = format!("{}x{}", img.width(), img.height());
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &size,
            "-i",
            "-",
            "-frames:v",
            "1",
            "-c:v",
            "libaom-av1",
            // cpu-used 8 is libaom's fastest setting; for a still frame it is
            // still visually clean at this CRF and keeps generation quick.
            "-cpu-used",
            "8",
            "-row-mt",
            "1",
            "-crf",
            "28",
            "-b:v",
            "0",
            "-f",
            "avif",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to launch ffmpeg: {e}"))?;
    // Write the frame, then drop stdin so FFmpeg sees end-of-input.
    {
        let mut stdin = child.stdin.take().expect("ffmpeg stdin is piped");
        stdin.write_all(img.as_raw())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        anyhow::bail!(
            "ffmpeg failed to encode {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Every fixture models the same realistic source: a 10-minute 1080p 10-bit
/// clip at 23.976 fps. 600 s x 23.976 fps is ~14386 frames.
const CLIP_FRAMES: usize = 14386;
const CLIP_WIDTH: usize = 1920;
const CLIP_HEIGHT: usize = 1080;
/// 23.976 fps as a float, for sizing estimates.
const CLIP_FPS: f64 = 24000.0 / 1001.0;

/// Deterministic-but-varied scene boundaries (~2 s to ~30 s) that tile the
/// whole clip, so detections look like a real run rather than uniform slices.
fn mock_scene_bounds() -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut start = 0usize;
    let mut seed = 0x9E37_79B9u32;
    while start < CLIP_FRAMES {
        // xorshift32: stable across runs, varied scene lengths.
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let len = 48 + (seed as usize % 672); // 48..720 frames (2..30 s)
        let end = (start + len).min(CLIP_FRAMES);
        bounds.push((start, end));
        start = end;
    }
    bounds
}

fn clip_info() -> ClipInfo {
    ClipInfo {
        num_frames:                    CLIP_FRAMES,
        format_info:                   PixelFormat::VapourSynth {
            bit_depth: 10
        },
        frame_rate:                    av_format::rational::Rational64::new(24000, 1001),
        resolution:                    (CLIP_WIDTH as u32, CLIP_HEIGHT as u32),
        color_range:                   None,
        color_primaries:               None,
        color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
        color_space:                   None,
        chroma_location:               None,
        mastering_display:             None,
        content_light_level:           None,
    }
}

fn encoder() -> Encoder {
    Encoder::default_from_base(&EncoderBase::SVTAV1, false)
}

/// A set of scene boundaries for fixtures (no real video required).
fn mock_scenes() -> Vec<Scene<CliSequenceData>> {
    let video = TestVideo {
        path:         PathBuf::new(),
        width:        CLIP_WIDTH,
        height:       CLIP_HEIGHT,
        frames:       CLIP_FRAMES,
        fps_rational: (24000, 1001),
        format:       FFPixelFormat::YUV420P,
        scenes:       mock_scene_bounds(),
    };
    video.mock_scenes(&encoder())
}

/// Move an app's wall clock into the past so the ETA / FPS read realistically.
fn backdate(started: &mut Instant, seconds: u64) {
    if let Some(past) = Instant::now().checked_sub(Duration::from_secs(seconds)) {
        *started = past;
    }
}

/// Set an app's wall clock so `frames` encoded at `fps` reads back correctly.
///
/// The progress bar derives FPS as `frames / elapsed`, so backdating by
/// `frames / fps` seconds makes the on-screen FPS match the value we chose.
fn backdate_for_frames(started: &mut Instant, frames: u64, fps: f64) {
    let seconds = (frames as f64 / fps).round() as u64;
    backdate(started, seconds.max(1));
}

/// Representative target-quality probe results (one pass per scene), with a
/// quantizer/score/bitrate that track scene complexity.
fn tq_passes(scenes: &[Scene<CliSequenceData>]) -> BTreeMap<u64, Vec<QualityPass>> {
    scenes
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let i = i as f64;
            let pass = QualityPass {
                scene:        i as u64,
                current_pass: 2,
                total_passes: 3,
                quantizer:    (i * 0.9).sin().mul_add(6.0, 30.0),
                score:        (i * 0.7).cos().mul_add(3.0, 92.0),
                bitrate:      (i * 1.3).sin().mul_add(1400.0, 4200.0),
            };
            (i as u64, vec![pass])
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

struct Screen {
    slug:    &'static str,
    /// The command whose prompt line heads the screenshot, e.g.
    /// `detect-scenes`.
    command: &'static str,
    buf:     Buffer,
}

/// Build every TUI screen, rendering under whatever theme is currently
/// installed (the caller installs a guard so light/dark colors are baked in).
fn tui_screens() -> Vec<Screen> {
    let ci = clip_info();
    let mut screens = Vec::new();

    // Scene Detection.
    // ~68% through a 10-minute clip; scene detection is decode-bound, so a few
    // hundred FPS is realistic. Scene count grows with frames processed.
    {
        let bounds = mock_scene_bounds();
        let frames_processed = 9850u64;
        let found: Vec<(u64, u64)> = bounds
            .iter()
            .take_while(|(_, end)| (*end as u64) <= frames_processed)
            .map(|(s, e)| (*s as u64, *e as u64))
            .collect();
        let scenes_len = found.len();
        let mut app = SceneDetectionApp::new(0, CLIP_FRAMES as u64, Vec::new(), ci);
        app.on_snapshot(SceneDetectionState {
            frames_processed,
            total_frames: CLIP_FRAMES as u64,
            scenes: found,
            scenes_len,
        });
        // Backdate after on_snapshot so the FPS reads ~350.
        backdate_for_frames(&mut app.started, frames_processed, 350.0);
        screens.push(Screen {
            slug:    "scene-detection",
            command: "detect-scenes",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Noise Detection.
    // Samples a handful of frames per scene, so it advances in scenes-per-
    // second, not frames; ~0.35 SPS is realistic for 1080p sampling.
    {
        let scenes = mock_scenes();
        let processed = 12u64;
        let mut app = NoiseDetectionApp::new(&scenes, ci);
        let scene_noise = scenes
            .iter()
            .enumerate()
            .take(processed as usize)
            .map(|(i, _)| (i as u64, (i as f64 * 0.7).sin().mul_add(6.0, 11.0)))
            .collect();
        app.on_snapshot(NoiseDetectionState {
            frames_processed: processed,
            total_frames: scenes.len() as u64,
            scene_noise,
        });
        // ~0.35 SPS => ~34s for 12 scenes.
        backdate_for_frames(&mut app.started, processed, 0.35);
        screens.push(Screen {
            slug:    "noise-detection",
            command: "detect-noise",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Benchmarker.
    // Mirrors the real loop: baseline at 1 worker, then add a worker at a time
    // while aggregate FPS keeps rising by the threshold. Worker 1 is the
    // baseline (never "added"), workers 2-5 meet threshold (checkmark), and
    // worker 6 fails to beat worker 5 by the threshold, so benchmarking stops
    // there (cross). Aggregate FPS shows diminishing returns (24 -> 95).
    {
        let mut app = BenchmarkerApp::new(encoder(), ci);
        let now = Instant::now();
        // Each worker encodes the same 480-frame benchmark sample; more workers
        // finish it sooner, so aggregate FPS rises with diminishing returns.
        let sample: u64 = 480;
        // (workers, aggregate FPS)
        let curve: [(u8, f64); 6] =
            [(1, 24.0), (2, 45.0), (3, 63.0), (4, 78.0), (5, 90.0), (6, 95.0)];
        let results = curve
            .into_iter()
            .map(|(workers, fps)| {
                let duration = Duration::from_secs_f64(sample as f64 / fps);
                // Workers 2-5 met the threshold and were added; worker 6 is the
                // baseline-meeting check that failed and stopped the run.
                let (added, failed_reason) = match workers {
                    1 => (false, None), // baseline: not "added", no failure
                    2..=5 => (true, None),
                    _ => (false, Some("Threshold not met".to_owned())),
                };
                (workers, WorkerStatus {
                    started: now - duration,
                    finished: Some(now),
                    added,
                    failed_reason,
                    current_frame: sample,
                    total_frames: sample,
                })
            })
            .collect();
        app.on_snapshot(BenchmarkerState {
            results,
        });
        screens.push(Screen {
            slug:    "benchmarker",
            command: "benchmark",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Target Quality — encoding phase.
    // Mid-way through pass 2 of 3. A single 1080p10-bit AV1 encode runs ~24
    // FPS, so encoding ~6000 frames takes ~4 minutes.
    {
        let scenes = mock_scenes();
        let passes = tq_passes(&scenes);
        let mut app = TargetQualityApp::new(ci, scenes, encoder(), ProbeStatistic::Mean);
        app.on_snapshot(TargetQualityState {
            quality_passes:  passes,
            current_pass:    2,
            frames_encoded:  6040,
            frames_compared: 0,
            total_frames:    CLIP_FRAMES as u64,
        });
        // Backdate after on_snapshot (which resets the pass timer) so FPS ~24.
        backdate_for_frames(&mut app.pass_started, 6040, 24.0);
        screens.push(Screen {
            slug:    "target-quality-encoding",
            command: "target-quality",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Target Quality — comparing phase.
    // All frames encoded; now scoring with VMAF, which runs far faster (~110
    // FPS) than the encode it measures.
    {
        let scenes = mock_scenes();
        let passes = tq_passes(&scenes);
        let mut app = TargetQualityApp::new(ci, scenes, encoder(), ProbeStatistic::Mean);
        app.on_snapshot(TargetQualityState {
            quality_passes:  passes,
            current_pass:    2,
            frames_encoded:  CLIP_FRAMES as u64,
            frames_compared: 7190,
            total_frames:    CLIP_FRAMES as u64,
        });
        // Backdate after on_snapshot so FPS ~110.
        backdate_for_frames(&mut app.pass_started, 7190, 110.0);
        screens.push(Screen {
            slug:    "target-quality-comparing",
            command: "target-quality",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Parallel Encoder.
    // A resumed 4-worker encode. `initial_map` is the state at app construction
    // (4 scenes already done); the snapshot is later, with 20 scenes done and 4
    // more actively encoding. The progress-bar FPS is derived from the frames
    // completed between those two points, so it reads as a real ~90 FPS aggregate.
    {
        let scenes = mock_scenes();
        let total: u64 = scenes.iter().map(|s| (s.end_frame - s.start_frame) as u64).sum();
        // Build a scene->completed map: fully done below `complete_to`, half-done
        // for the active window, zero otherwise.
        let mk_map = |complete_to: usize, active: std::ops::Range<usize>| {
            scenes
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let len = (s.end_frame - s.start_frame) as u64;
                    let completed = if i < complete_to {
                        len
                    } else if active.contains(&i) {
                        len / 2
                    } else {
                        0
                    };
                    (i as u64, (completed, s.clone()))
                })
                .collect::<BTreeMap<_, _>>()
        };
        let initial_map = mk_map(4, 0..0); // resume point: 4 scenes already done
        let snapshot_map = mk_map(20, 20..24); // 20 done + 4 active at ~50%
        let mut app = ParallelEncoderApp::new(4, encoder(), initial_map, ci);
        let active = snapshot_map
            .iter()
            .filter(|(i, _)| (20u64..24).contains(i))
            .map(|(idx, (processed, s))| {
                let mut se = SceneEncoder {
                    scene:            s.clone(),
                    started:          Instant::now(),
                    current_pass:     1,
                    total_passes:     2,
                    frames_processed: *processed,
                    total_frames:     (s.end_frame - s.start_frame) as u64,
                };
                // Each worker ~24 FPS on its own scene.
                backdate_for_frames(&mut se.started, *processed, 24.0);
                (*idx, se)
            })
            .collect();
        let snapshot_completed: u64 = snapshot_map.iter().map(|(_, (c, _))| c).sum();
        let delta = snapshot_completed.saturating_sub(app.initial_frames);
        let est_bitrate = 3_500_000.0; // ~3500 kbps is realistic for 1080p AV1
        let est_bytes = (est_bitrate / 8.0 * (total as f64 / CLIP_FPS)) as u64;
        app.on_snapshot(ParallelEncoderState {
            scenes:                 snapshot_map,
            active_encoders:        active,
            clip_info:              ci,
            completed_scenes_count: 20,
            estimated_bitrate:      est_bitrate,
            estimated_bytes:        est_bytes,
        });
        // Overall aggregate ~90 FPS across the active workers since resume.
        backdate_for_frames(&mut app.started, delta, 90.0);
        screens.push(Screen {
            slug:    "parallel-encoder",
            command: "encode",
            buf:     capture(|f| app.render(f)),
        });
    }

    // Scene Concatenator.
    {
        let mut app = SceneConcatenatorApp::new(ci, 48, ConcatMethod::Ivf);
        backdate(&mut app.started, 33);
        app.on_snapshot(SceneConcatenatorState {
            percent: 72.0
        });
        screens.push(Screen {
            slug:    "scene-concatenator",
            command: "concatenate",
            buf:     capture(|f| app.render(f)),
        });
    }

    screens
}

/// The kebab-case names of every subcommand.
const SUBCOMMANDS: &[&str] = &[
    "init",
    "detect-scenes",
    "detect-noise",
    "scale-noise",
    "benchmark",
    "target-quality",
    "optimize-bitrate",
    "scale-speed",
    "encode",
    "concatenate",
    "quality-check",
    "clean",
];

/// Apply one SGR parameter string (the text between `ESC[` and `m`) to a style.
///
/// `--version` output is ANSI-styled by ironmark, so the harness needs just
/// enough of a terminal emulator to carry those colours into the buffer.
fn apply_sgr(style: Style, seq: &str) -> Style {
    let params: Vec<u16> = if seq.is_empty() {
        vec![0]
    } else {
        seq.split(';').map(|p| p.parse().unwrap_or(0)).collect()
    };

    let mut style = style;
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            30..=37 => style = style.fg(Color::Indexed((params[i] - 30) as u8)),
            90..=97 => style = style.fg(Color::Indexed((params[i] - 90 + 8) as u8)),
            39 => style = style.fg(Color::Reset),
            40..=47 => style = style.bg(Color::Indexed((params[i] - 40) as u8)),
            100..=107 => style = style.bg(Color::Indexed((params[i] - 100 + 8) as u8)),
            49 => style = style.bg(Color::Reset),
            38 | 48 => {
                // The two extended forms: `38;5;n` (256-colour) and
                // `38;2;r;g;b` (truecolour); the parameter count distinguishes
                // them.
                let (color, consumed) = match params.get(i + 1) {
                    Some(5) => match params.get(i + 2) {
                        Some(&idx) => (Some(Color::Indexed(idx as u8)), 2),
                        None => (None, 0),
                    },
                    Some(2) => match (params.get(i + 2), params.get(i + 3), params.get(i + 4)) {
                        (Some(&r), Some(&g), Some(&b)) => {
                            (Some(Color::Rgb(r as u8, g as u8, b as u8)), 4)
                        },
                        _ => (None, 0),
                    },
                    _ => (None, 0),
                };
                if let Some(color) = color {
                    i += consumed;
                    style = if params[i - consumed] == 38 {
                        style.fg(color)
                    } else {
                        style.bg(color)
                    };
                }
            },
            _ => {},
        }
        i += 1;
    }
    style
}

/// Split ANSI-styled terminal output into styled ratatui lines.
///
/// Only SGR (`ESC[...m`) sequences carry styling; any other escape sequence is
/// consumed and dropped rather than leaking escape bytes into the buffer.
fn ansi_lines(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut pending = String::new();
    let mut style = Style::default();
    let mut chars = text.chars().peekable();

    // Push the pending text under the current style, keeping spans split at
    // every style change so each run carries its own attributes.
    macro_rules! flush {
        () => {
            if !pending.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut pending), style));
            }
        };
    }

    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {},
            '\n' => {
                flush!();
                lines.push(Line::from(std::mem::take(&mut spans)));
            },
            '\x1b' => match chars.peek() {
                // CSI (e.g. an SGR colour): consume up to the final byte.
                Some(&'[') => {
                    chars.next();
                    let mut seq = String::new();
                    let mut terminator = ' ';
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            terminator = c;
                            break;
                        }
                        seq.push(c);
                    }
                    if terminator == 'm' {
                        flush!();
                        style = apply_sgr(style, &seq);
                    }
                },
                // OSC (e.g. `ESC]8;;URL` hyperlinks): consume up to BEL or the
                // ST (`ESC \`) sequence so no escape bytes leak as text.
                Some(&']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                },
                // Any other escape: drop the escape and the byte that follows.
                _ => {
                    chars.next();
                },
            },
            _ => pending.push(ch),
        }
    }
    flush!();
    // A trailing newline ends the last line; it does not open a new one.
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// Compose a terminal frame: the PowerShell prompt line that launched the
/// command on top, then the interface itself below it. This mirrors how the
/// screenshots are taken on a real machine — the prompt is what you typed
/// immediately before the TUI took over the screen.
fn session_buf(screen: &Screen, theme: Theme) -> Buffer {
    let mut canvas = Buffer::empty(Rect::new(0, 0, COLS, ROWS));

    // Row 0: `PS C:\Condor> .\condor.exe <command>` in PowerShell colours —
    // the prompt is what was typed immediately before the TUI took over.
    let prompt_fg = theme.foreground;
    let spans = [
        Span::styled("PS ", Style::default().fg(theme.dim)),
        Span::styled("C:\\Condor> ", Style::default().fg(prompt_fg)),
        Span::styled(".\\condor.exe", Style::default().fg(theme.main)),
        Span::styled(
            format!(" {}", screen.command),
            Style::default().fg(prompt_fg),
        ),
    ];
    canvas.set_style(
        Rect::new(0, 0, COLS, 1),
        Style::default().bg(theme.background),
    );
    Line::from(spans.to_vec()).render(Rect::new(0, 0, COLS, 1), &mut canvas);

    // Rows 1..: the interface itself, shifted down one row.
    for y in 0..ROWS - 1 {
        for x in 0..COLS {
            canvas[(x, y + 1)] = screen.buf[(x, y)].clone();
        }
    }
    canvas
}

fn boxed_screen(
    slug: String,
    command: &'static str,
    title: String,
    lines: Vec<Line<'static>>,
) -> Screen {
    use ratatui::widgets::{Block, BorderType, Paragraph};
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(title).centered())
        .title_bottom(Line::from(" California Condor ").centered());
    let para = Paragraph::new(lines).block(block);
    Screen {
        slug: Box::leak(slug.into_boxed_str()),
        command,
        buf: capture(move |f| {
            let area = f.area();
            f.render_widget(para, area);
        }),
    }
}

fn help_screen(slug: String, command: &'static str, title: String, text: &str) -> Screen {
    let lines: Vec<Line> = text.lines().map(|l| Line::from(l.to_owned())).collect();
    boxed_screen(slug, command, title, lines)
}

fn help_screens() -> Vec<Screen> {
    let mut screens = Vec::new();

    {
        let mut cmd = CondorCli::command();
        let short = cmd.render_help().to_string();
        screens.push(help_screen(
            "help".into(),
            "--help",
            "condor --help".into(),
            &short,
        ));
    }
    {
        let mut cmd = CondorCli::command();
        let long = cmd.render_long_help().to_string();
        screens.push(help_screen(
            "help-verbose".into(),
            "--help --verbose",
            "condor --help --verbose".into(),
            &long,
        ));
    }

    for name in SUBCOMMANDS {
        let mut cmd = CondorCli::command();
        if let Some(sub) = cmd.find_subcommand_mut(name) {
            let short = sub.clone().render_help().to_string();
            let command = Box::leak(format!("{name} --help").into_boxed_str());
            screens.push(help_screen(
                format!("help-{name}"),
                command,
                format!("condor {name} --help"),
                &short,
            ));
        }
        let mut cmd = CondorCli::command();
        if let Some(sub) = cmd.find_subcommand_mut(name) {
            let long = sub.clone().render_long_help().to_string();
            let command = Box::leak(format!("{name} --help --verbose").into_boxed_str());
            screens.push(help_screen(
                format!("help-{name}-verbose"),
                command,
                format!("condor {name} --help --verbose"),
                &long,
            ));
        }
    }

    screens
}

fn version_screens() -> Vec<Screen> {
    [
        (false, "version", "--version", "condor --version"),
        (
            true,
            "version-verbose",
            "--version --verbose",
            "condor --version --verbose",
        ),
    ]
    .into_iter()
    .map(|(verbose, slug, command, title)| {
        // Rendered by the same function the CLI prints, so the image cannot
        // drift from real output.
        let text = render_version(verbose).expect("render --version");
        boxed_screen(
            slug.to_owned(),
            command,
            title.to_owned(),
            ansi_lines(&text),
        )
    })
    .collect()
}

/// The version report is prose whose height depends on the machine's installed
/// plugins, encoders and devices. Guard that it still fits the canvas, so a
/// machine with many devices fails loudly instead of shipping a screenshot
/// with the bottom rows silently cut off.
fn check_version_fits() -> anyhow::Result<()> {
    // Row 0 is the prompt line, so the interface itself gets ROWS - 1 rows.
    let rows_available = ROWS - 1;
    for verbose in [false, true] {
        let rows = render_version(verbose)?.lines().count() as u16;
        if rows > rows_available {
            anyhow::bail!(
                "the --version{} report is {rows} rows tall, taller than the {rows_available}-row \
                 interface area; shorten the report or grow ROWS/CELL_H together",
                if verbose { " --verbose" } else { "" }
            );
        }
    }
    Ok(())
}

fn suffix(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Render every screen in both themes and write AVIFs into `out_dir`.
/// Returns the list of `(file_name, width, height)` written.
pub fn generate_all(out_dir: &Path) -> anyhow::Result<Vec<(String, u32, u32)>> {
    check_version_fits()?;
    let font = FontVec::try_from_vec(load_font()).expect("parse font");
    let mut written = Vec::new();
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let theme = Theme::for_mode(mode);
        let _guard = Theme::set_mode(mode);
        let screens = tui_screens().into_iter().chain(help_screens()).chain(version_screens());
        for screen in screens {
            let img = rasterize(&session_buf(&screen, theme), theme, &font);
            let name = format!("{}-{}.avif", screen.slug, suffix(mode));
            encode_avif(&img, &out_dir.join(&name))?;
            written.push((name, WIDTH, HEIGHT));
        }
    }
    Ok(written)
}

/// Render every screen in both themes in memory and assert the dimensions.
/// This is the integration-test smoke check: it exercises the real render code
/// for every screen and would surface any panic or layout change.
pub fn verify_all() -> anyhow::Result<()> {
    check_version_fits()?;
    let font = FontVec::try_from_vec(load_font()).expect("parse font");
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let theme = Theme::for_mode(mode);
        let _guard = Theme::set_mode(mode);
        for screen in tui_screens()
            .iter()
            .chain(help_screens().iter())
            .chain(version_screens().iter())
        {
            let img = rasterize(&session_buf(screen, theme), theme, &font);
            assert_eq!(img.width(), WIDTH, "width for {}", screen.slug);
            assert_eq!(img.height(), HEIGHT, "height for {}", screen.slug);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Color, text::Span};

    use super::{ansi_lines, draw_tiling_char};

    /// A run of box-drawing or block glyphs must paint every pixel of the
    /// band it belongs to — no gaps between cells — or borders render as
    /// dashes and gauges as stripes.
    #[test]
    fn box_drawing_and_blocks_tile_without_gaps() {
        for (ch, describe) in [
            ('\u{2500}', "horizontal"),
            ('\u{2501}', "heavy horizontal"),
            ('\u{2588}', "full block"),
        ] {
            let mut img =
                super::Canvas::from_pixel(super::CELL_W * 3, super::CELL_H, [0, 0, 0, 255]);
            for cell in 0..3 {
                assert!(
                    draw_tiling_char(&mut img, ch, cell * super::CELL_W, 0, (255, 255, 255)),
                    "{ch:?} should be handled by the tiling painter"
                );
            }
            // Every pixel of the band is painted across cell borders: the
            // middle rows for a line, the whole cell for a block.
            let band_start = if ch == '\u{2588}' {
                0
            } else {
                super::CELL_H / 2 - 1
            };
            let band_end = if ch == '\u{2588}' {
                super::CELL_H
            } else {
                band_start + 2
            };
            for y in band_start..band_end {
                for x in 0..super::CELL_W * 3 {
                    let p = img.get_pixel(x, y);
                    assert_eq!(
                        p,
                        [255, 255, 255, 255],
                        "{describe}: pixel ({x},{y}) in the tiled band is not painted"
                    );
                }
            }
        }
    }

    /// A double line is two hairlines, each seamless across cell borders.
    #[test]
    fn double_lines_tile_without_gaps() {
        let mut img = super::Canvas::from_pixel(super::CELL_W * 3, super::CELL_H, [0, 0, 0, 255]);
        for cell in 0..3 {
            assert!(draw_tiling_char(
                &mut img,
                '\u{2550}',
                cell * super::CELL_W,
                0,
                (255, 255, 255)
            ));
        }
        let cy = super::CELL_H / 2;
        for y in [cy - 2, cy + 1] {
            for x in 0..super::CELL_W * 3 {
                assert_eq!(
                    img.get_pixel(x, y),
                    [255, 255, 255, 255],
                    "double: hairline pixel ({x},{y}) is not painted"
                );
            }
        }
    }

    /// Vertical runs tile top-to-bottom the same way.
    #[test]
    fn vertical_box_drawing_tiles_without_gaps() {
        let mut img = super::Canvas::from_pixel(super::CELL_W, super::CELL_H * 3, [0, 0, 0, 255]);
        for cell in 0..3 {
            assert!(draw_tiling_char(
                &mut img,
                '\u{2502}',
                0,
                cell * super::CELL_H,
                (255, 255, 255)
            ));
        }
        let xmid = super::CELL_W / 2;
        for y in 0..super::CELL_H * 3 {
            for x in xmid - 1..xmid + 1 {
                assert_eq!(
                    img.get_pixel(x, y),
                    [255, 255, 255, 255],
                    "vertical: pixel ({x},{y}) in the tiled band is not painted"
                );
            }
        }
    }

    /// OSC hyperlinks (`ESC]8;;URL`) are consumed, not leaked as literal
    /// `]8;;` text, in both the BEL- and ST-terminated forms.
    #[test]
    fn osc_hyperlinks_are_dropped() {
        let lines = ansi_lines("\x1b]8;;https://example.com\x07link\x1b]8;;\x07 rest");
        let spans: Vec<&Span> = lines[0].spans.iter().collect();
        assert_eq!(spans.len(), 1, "expected one run: {spans:?}");
        assert_eq!(spans[0].content.as_ref(), "link rest");

        let lines = ansi_lines("\x1b]8;;https://example.com\x1b\\link");
        assert_eq!(lines[0].spans[0].content.as_ref(), "link");
    }

    /// The real `condor --version --verbose` report (rendered by ironmark,
    /// which emits OSC 8 hyperlinks) must not leak escape bytes like `]8;;`
    /// into the buffer — the reported screenshot bug.
    #[test]
    fn version_report_has_no_escape_leak() {
        let text = super::render_version(true).expect("render --version --verbose");
        for (i, line) in ansi_lines(&text).iter().enumerate() {
            let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                !rendered.contains("]8;;") && !rendered.contains('\x1b'),
                "escape leaked on line {i}: {rendered:?}"
            );
        }
    }

    /// A styled run and an unstyled run split at the style change, with the
    /// reset carrying through to the trailing text.
    #[test]
    fn sgr_colors_are_carried_into_spans() {
        let lines = ansi_lines("\x1b[0;32m\u{2713} green\x1b[0m plain");
        let spans = lines[0].spans.as_slice();
        assert_eq!(spans.len(), 2, "expected two styled runs: {spans:?}");
        assert_eq!(spans[0].content.as_ref(), "\u{2713} green");
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(2)));
        assert_eq!(spans[1].content.as_ref(), " plain");
        assert_eq!(spans[1].style.fg, None);
    }

    /// Both extended SGR colour forms (`38;5;n` and `38;2;r;g;b`) resolve to
    /// the colours the rasterizer understands.
    #[test]
    fn extended_colours_resolve() {
        let lines = ansi_lines("\x1b[38;5;120m256\x1b[38;2;10;20;30mtrue\x1b[0m plain");
        let spans = lines[0].spans.as_slice();
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(120)));
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(spans[2].style.fg, None);
    }

    /// Non-SGR escapes (cursor moves, clears) must not leak into the buffer as
    /// literal text — they are consumed and dropped.
    #[test]
    fn non_sgr_escapes_are_dropped() {
        let lines = ansi_lines("a\x1b[2Jb\x1b[10;20Hc");
        let spans: Vec<&Span> = lines[0].spans.iter().collect();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "abc");
    }

    /// Newlines split lines; a trailing newline does not add an empty one.
    #[test]
    fn newlines_split_lines() {
        let lines = ansi_lines("one\ntwo\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content.as_ref(), "one");
        assert_eq!(lines[1].spans[0].content.as_ref(), "two");
    }
}
