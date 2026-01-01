use av1an_core::condor::data::encoding::Encoder;
use ratatui::{
    text::Line,
    widgets::{Block, Paragraph},
};

pub struct EncoderInfo {
    pub encoder:        Encoder,
    pub parent_encoder: Option<Encoder>,
}

impl EncoderInfo {
    #[inline]
    pub fn new(encoder: Encoder, parent_encoder: Option<Encoder>) -> Self {
        Self {
            encoder,
            parent_encoder,
        }
    }

    #[inline]
    pub fn generate(&self, bordered: bool) -> Paragraph<'_> {
        let mut scene_parameters = self.encoder.parameters().to_owned();
        if let Some(parent_encoder) = &self.parent_encoder {
            for (key, parent_value) in parent_encoder.parameters() {
                if let Some(value) = scene_parameters.get(key)
                    && value == parent_value
                {
                    scene_parameters.remove(key);
                }
            }
        }
        if scene_parameters.is_empty() {
            scene_parameters = self.encoder.parameters().to_owned();
        }

        let parameters_text = scene_parameters
            .iter()
            .map(|(key, value)| value.to_parameter_string(key))
            .collect::<Vec<_>>()
            .join(" ");

        let encoder_paragraph = Paragraph::new(Line::from(parameters_text));
        let encoder_paragraph = if bordered {
            let encoder_name = self.encoder.base().friendly_name();
            encoder_paragraph.block(Block::bordered().title(Line::from(encoder_name).centered()))
        } else {
            encoder_paragraph
        };
        encoder_paragraph.centered()
    }
}
