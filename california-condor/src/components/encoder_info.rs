use andean_condor::models::encoder::Encoder;
use ratatui::{
    text::Line,
    widgets::{Block, Paragraph, Wrap},
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

        let encoder_paragraph = if !scene_parameters.is_empty() {
            let parameters_text = scene_parameters
                .iter()
                .map(|(key, value)| value.to_parameter_string(key))
                .collect::<Vec<_>>()
                .join(" ");

            Paragraph::new(Line::from(parameters_text)).wrap(Wrap {
                trim: true
            })
        } else {
            Paragraph::new(Line::from("---"))
                .wrap(Wrap {
                    trim: true
                })
                .centered()
        };

        if bordered {
            let encoder_name = self.encoder.base().friendly_name();
            encoder_paragraph.block(Block::bordered().title(Line::from(encoder_name).centered()))
        } else {
            encoder_paragraph
        }
    }
}
