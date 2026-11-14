use andean_condor::models::encoder::Encoder;
use ratatui::{
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};

/// The complete uniform scene configuration, or `None` for mixed settings.
/// Only an empty scene list uses the global fallback.
pub(crate) fn representative_encoder<'a>(
    mut encoders: impl Iterator<Item = &'a Encoder>,
    fallback: &'a Encoder,
) -> Option<&'a Encoder> {
    let first = encoders.next().unwrap_or(fallback);
    encoders.all(|encoder| encoder == first).then_some(first)
}

pub struct EncoderInfo {
    pub encoder: Encoder,
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
        if let Some(parent_encoder) = self
            .parent_encoder
            .as_ref()
            .filter(|parent| parent.base() == self.encoder.base())
        {
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

            Paragraph::new(Line::from(parameters_text)).wrap(Wrap { trim: true })
        } else {
            Paragraph::new(Line::from("---")).wrap(Wrap { trim: true }).centered()
        };

        if bordered {
            let encoder_name = self.encoder.base().friendly_name();
            encoder_paragraph.block(Block::bordered().title(Line::from(encoder_name).centered()))
        } else {
            encoder_paragraph
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use andean_condor::models::encoder::EncoderBase;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn parameter_diff_only_hides_equal_options_for_the_same_codec() {
        let parent = Encoder::default_from_base(&EncoderBase::X264, false);
        for base in [EncoderBase::X264, EncoderBase::X265] {
            let actual = Encoder::default_from_base(&base, false);
            let info = EncoderInfo::new(actual, Some(parent.clone()));
            let mut terminal = Terminal::new(TestBackend::new(120, 5)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(info.generate(false), frame.area()))
                .unwrap();
            let buf = terminal.backend().buffer();
            let text: String = buf.content().iter().map(|cell| cell.symbol()).collect();
            assert_eq!(
                text.contains("--crf 25"),
                base == EncoderBase::X265,
                "{text}"
            );
            if base == EncoderBase::X264 {
                assert!(
                    text.contains("---"),
                    "same-codec diff remains compact: {text}"
                );
            }
        }
    }
}
