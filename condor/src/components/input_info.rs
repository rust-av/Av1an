use av1an_core::ClipInfo;
use ratatui::{
    text::Line,
    widgets::{Block, Paragraph},
};

pub struct InputInfo {
    pub clip_info: ClipInfo,
}

impl InputInfo {
    #[inline]
    pub fn new(clip_info: ClipInfo) -> Self {
        Self {
            clip_info,
        }
    }

    #[inline]
    pub fn generate(&self, bordered: bool) -> Paragraph<'_> {
        let input_info = format!(
            "{}x{} {:.3}FPS {}-bit {}",
            self.clip_info.resolution.0,
            self.clip_info.resolution.1,
            (*self.clip_info.frame_rate.numer() as f64 / *self.clip_info.frame_rate.denom() as f64),
            self.clip_info.format_info.as_bit_depth().unwrap_or_default(),
            match self.clip_info.transfer_characteristics {
                av1an_core::TransferFunction::BT1886 => "SDR",
                av1an_core::TransferFunction::SMPTE2084 => "HDR",
            }
        );
        let input_paragraph = Paragraph::new(Line::from(input_info));
        let input_paragraph = if bordered {
            input_paragraph.block(Block::bordered().title(Line::from("Input").centered()))
        } else {
            input_paragraph
        };
        input_paragraph.centered()
    }
}
