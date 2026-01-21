use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;


pub struct LogMerger {
    encoder: String,
    total_seconds: f64,
    // Frame type stats: (count, total_qp, total_size_or_rate)
    // For x264, 3rd element is Bytes. For x265, strict parsing is harder for rate, 
    // but x265 reports 'kb/s' per type. We can try to sum kb/s * time? 
    // Or just parse whatever the 3rd metric is.
    // x264 'size' is Bytes. x265 'kb/s' is Rate.
    frame_i_stats: (usize, f64, f64),
    frame_p_stats: (usize, f64, f64),
    frame_b_stats: (usize, f64, f64),
    
    // Header lines to preserve (from first chunk)
    headers: Vec<String>,
    
    // Captured log prefix (e.g. "x264 [info]: ")
    log_prefix: String,
    
    // Flag to stop collecting headers once encoding starts
    header_done: bool,
}

impl LogMerger {
    pub fn new(encoder: &str) -> Self {
        Self {
            encoder: encoder.to_string(),
            total_seconds: 0.0,
            frame_i_stats: (0, 0.0, 0.0),
            frame_p_stats: (0, 0.0, 0.0),
            frame_b_stats: (0, 0.0, 0.0),
            headers: Vec::new(),
            log_prefix: String::new(),
            header_done: false,
        }
    }

    pub fn process_chunk(&mut self, log_path: &Path, is_first: bool) -> io::Result<()> {
        let file = File::open(log_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            let trim = line.trim();

            // Skip garbage / progress lines strictly
            if trim.contains("Frames") || trim.contains("time=") || trim.contains("FPS") {
                continue;
            }
            
            let is_stat = if self.encoder.contains("x265") {
                self.parse_x265_line(trim)
            } else {
                self.parse_x264_line(trim)
            };

            if is_stat {
                self.header_done = true;
                continue;
            }
            
            // Capture headers
            if is_first && !self.header_done {
                // Heuristic: Keep lines that look like config/headers.
                // Exclude known stats/progress patterns if they slipped through
                if !trim.contains("frame I:") && 
                   !trim.contains("frame P:") && 
                   !trim.contains("frame B:") && 
                   !trim.contains("encoded") &&
                   !trim.contains("Weighted") &&
                   !trim.contains("consecutive B-frames") &&
                   !trim.contains("kb/s:") &&
                   !trim.is_empty() 
                {
                     self.headers.push(line.clone());
                }
            }
        }
        Ok(())
    }

    fn parse_x265_line(&mut self, line: &str) -> bool {
        // Robust x265 parser using anchors
        // source: "frame %c: %6u, Avg QP:%2.2lf  kb/s: %s"
        // Example: "x265 [info]: frame I:    473, Avg QP: 8.92  kb/s: 56843.55"

        let matched = self.parse_stats_generic(line, true);
        if matched {
             // Debug print if needed
        }
        matched
    }

    fn parse_x264_line(&mut self, line: &str) -> bool {
        // Robust x264 parser
        // source: "frame %c:%-5d Avg QP:%5.2f  size:%6.0f"
        // Example: "x264 [info]: frame I:17    Avg QP:15.35  size: 38890"
        
        // Similar generic parsing but expect "size" instead of "kb/s" and handle accumulated bytes
        self.parse_stats_generic(line, false)
    }

    fn parse_stats_generic(&mut self, line: &str, is_x265: bool) -> bool {
        let mut matched = false;
        
        for &type_char in &['I', 'P', 'B'] {
            let tag = format!("frame {}:", type_char);
            if let Some(mut idx) = line.find(&tag) {
                // Found frame tag.
                idx += tag.len();
                let remainder = &line[idx..];

                // Strategy: Extract number up to next alpha character or comma or colon
                // x265: "    473, Avg QP..."
                // x264: "17    Avg QP..."
                
                let count_str: String = remainder.chars()
                    .take_while(|c| c.is_ascii_digit() || c.is_whitespace()).collect();
                
                // If comma exists in count_str, strip it? no, take_while digit/space won't take comma.
                // Wait. x265 has comma. "473, "
                // If I take digits/space, I stop at comma. Correct.
                
                let trimmed_count = count_str.trim();
                let count = if let Ok(n) = trimmed_count.parse::<usize>() { n } else { continue };

                // QP Anchor
                // x265: "Avg QP:" . x264: "Avg QP:" or "QP:"
                let qp_anchor = if remainder.contains("Avg QP:") { "Avg QP:" } else { "QP:" };
                let qp = if let Some(qp_idx) = remainder.find(qp_anchor) {
                    let s = &remainder[qp_idx+qp_anchor.len()..];
                    // Parse float
                    let qp_val_str: String = s.trim_start().chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    qp_val_str.parse().unwrap_or(0.0)
                } else { 0.0 };

                // Rate/Size Anchor
                let (metric_val, is_rate) = if let Some(kb_idx) = remainder.find("kb/s:") {
                    let s = &remainder[kb_idx+5..];
                    let val_str: String = s.trim_start().chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    (val_str.parse().unwrap_or(0.0), true)
                } else if let Some(sz_idx) = remainder.find("size:") {
                    let s = &remainder[sz_idx+5..];
                    let val_str: String = s.trim_start().chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    (val_str.parse().unwrap_or(0.0), false)
                } else {
                    (0.0, is_x265)
                };

                if matches!(type_char, 'I') {
                    self.capture_prefix(line, &tag);
                }

                // Store
                let stats = match type_char {
                    'I' => &mut self.frame_i_stats,
                    'P' => &mut self.frame_p_stats,
                    'B' => &mut self.frame_b_stats,
                    _ => unreachable!(),
                };
                
                stats.0 += count;
                stats.1 += qp * count as f64;
                
                if is_rate {
                    // x265 rate case
                    stats.2 += metric_val * count as f64;
                } else {
                    // x264 size case (bytes)
                    stats.2 += metric_val;
                }
                
                matched = true;
                break; // Found the type for this line
            }
        }
        
        if !matched && line.contains("encoded") && line.contains("frames") {
            self.parse_summary(line);
            matched = true;
        }

        matched
    }

    fn capture_prefix(&mut self, line: &str, tag: &str) {
        if self.log_prefix.is_empty() {
            if let Some(idx) = line.find(tag) {
                self.log_prefix = line[..idx].trim().to_string();
            }
        }
    }

    fn parse_summary(&mut self, line: &str) {
        // Common parsers for time/fps to help reconstruct global stats if needed
        // x265: encoded 100 frames in 10.00s (10.00 fps), 1000.00 kb/s...
        // x264: encoded 100 frames, 10.00 fps, 1000.00 kb/s... (duration inferred?)
        
        let mut frames = 0;
        if let Some(idx) = line.find("encoded") {
            let s = &line[idx+7..]; // after "encoded"
            let frames_str: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(f) = frames_str.parse::<usize>() {
                 frames = f;
            }
        }
        
        // Extract duration/FPS to accumulate total_seconds
        // x265: "in 123.45s"
        if let Some(in_idx) = line.find("in ") {
            let s = &line[in_idx+3..];
            let time_str: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(time) = time_str.parse::<f64>() {
                self.total_seconds += time;
            }
        } else if let Some(fps_idx) = line.find("fps") {
            // fallback for x264 which often puts frame count and fps
            // "1684 frames, 35.53 fps"
            // We need frame count for *this chunk* to get duration. 
            // Parsing frame count again from this line:
             if frames > 0 {
                 let before_fps = &line[..fps_idx];
                 let fps_str: String = before_fps.trim().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                 let fps_str: String = fps_str.chars().rev().collect();
                 if let Ok(fps) = fps_str.parse::<f64>() {
                     if fps > 0.0 {
                         self.total_seconds += frames as f64 / fps;
                     }
                 }
             }
        }
        
        // Accumulate size?
        // WE already accumulated in parse_stats.
    }

    pub fn write_merged<W: Write>(&self, mut writer: W) -> io::Result<()> {
        for line in &self.headers {
            writeln!(writer, "{}", line)?;
        }
        writeln!(writer, "")?; // Spacing after headers

        let prefix = if self.log_prefix.is_empty() {
             if self.encoder.contains("x265") { "x265 [info]:" } else { "x264 [info]:" }
        } else {
             &self.log_prefix
        };

        // Write Frame Stats
        // x265: "frame I:    473, Avg QP: 8.92  kb/s: 56843.55"
        // x264: "frame I:17    Avg QP:15.35  size: 38890"

        let write_stat = |w: &mut W, type_char: char, s: (usize, f64, f64)| -> io::Result<()> {
            if s.0 == 0 { return Ok(()); }
            let avg_qp = s.1 / s.0 as f64;
            
            if self.encoder.contains("x265") {
                let avg_rate = s.2 / s.0 as f64;
                writeln!(w, "{} frame {}: {:>8}, Avg QP: {:>5.2}  kb/s: {:>8.2}", prefix, type_char, s.0, avg_qp, avg_rate)?;
            } else {
                let total_size = s.2 as u64;
                writeln!(w, "{} frame {}:{:>5}    Avg QP:{:>5.2}  size: {:>8}", prefix, type_char, s.0, avg_qp, total_size)?;
            }
            Ok(())
        };

        write_stat(&mut writer, 'I', self.frame_i_stats)?;
        write_stat(&mut writer, 'P', self.frame_p_stats)?;
        write_stat(&mut writer, 'B', self.frame_b_stats)?;
        
        writeln!(writer, "")?;

        // Summary
        let total_frames = self.frame_i_stats.0 + self.frame_p_stats.0 + self.frame_b_stats.0;
        let total_time = self.total_seconds;
        let fps = if total_time > 0.0 { total_frames as f64 / total_time } else { 0.0 };
        
        // Calculate global bitrate
        // x265 accumulated Rate*Count in .2. -> Sum(Rate*Count) / TotalFrames = AvgRate.
        // x264 accumulated Bytes in .2. -> Sum(Bytes) * 8 / 1000 / TotalTime = Bitrate (kbps).
        
        let kbps = if self.encoder.contains("x265") {
             // Weighted average of rates
             let sum_rate_count = self.frame_i_stats.2 + self.frame_p_stats.2 + self.frame_b_stats.2;
             if total_frames > 0 { sum_rate_count / total_frames as f64 } else { 0.0 }
        } else {
             let total_bytes = self.frame_i_stats.2 + self.frame_p_stats.2 + self.frame_b_stats.2;
             if total_time > 0.0 { (total_bytes * 8.0) / 1000.0 / total_time } else { 0.0 }
        };

        let avg_qp = if total_frames > 0 {
            (self.frame_i_stats.1 + self.frame_p_stats.1 + self.frame_b_stats.1) / total_frames as f64
        } else { 0.0 };

        if self.encoder.contains("x265") {
            // encoded 33138 frames in 9989.67s (3.32 fps), 20845.84 kb/s, Avg QP:11.15
            writeln!(writer, "encoded {} frames in {:.2}s ({:.2} fps), {:.2} kb/s, Avg QP:{:.2}",
                total_frames, total_time, fps, kbps, avg_qp)?;
        } else {
            // encoded 1684 frames, 35.53 fps, 2732.18 kb/s
             writeln!(writer, "{} encoded {} frames, {:.2} fps, {:.2} kb/s",
                prefix, total_frames, fps, kbps)?;
        }

        Ok(())
    }
}
