use andean_condor::models::encoder::Encoder;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Color,
    text::Line,
    widgets::{Block, Widget},
};

use crate::{
    apps::parallel_encoder::SceneEncoder,
    components::{encoder_info::EncoderInfo, progress_bar::ProgressBar},
};

pub struct ActiveEncoders {
    pub color:          Color,
    pub workers:        u8,
    pub parent_encoder: Encoder,
    pub active_scenes:  Vec<(u64, SceneEncoder)>,
}

impl Widget for ActiveEncoders {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let worker_areas = Layout::default()
            .constraints(std::iter::repeat_n(
                Constraint::Fill(1),
                self.workers as usize,
            ))
            .split(area);
        for (worker_area, (scene_index, scene_encoder)) in
            worker_areas.iter().rev().zip(self.active_scenes.iter().rev())
        {
            let bordered_area = Block::bordered()
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title(Line::from(format!(
                    "Scene {} Pass {}/{}",
                    scene_index, scene_encoder.current_pass, scene_encoder.total_passes
                )));
            let worker_area_inner = bordered_area.inner(*worker_area);
            bordered_area.render(*worker_area, buf);
            let active_worker_areas = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)])
                .split(worker_area_inner);

            let encoder_info = EncoderInfo::new(
                scene_encoder.scene.encoder.clone(),
                Some(self.parent_encoder.clone()),
            );
            encoder_info.generate(false).render(active_worker_areas[0], buf);

            let progress_bar = ProgressBar {
                color:             self.color,
                processing_title:  String::new(),
                completed_title:   String::new(),
                initial_completed: 0,
                unit_per_second:   "FPS".to_owned(),
                unit:              "Frame".to_owned(),
                completed:         scene_encoder.frames_processed,
                total:             scene_encoder.total_frames,
            };

            progress_bar
                .generate(Some(scene_encoder.started))
                .render(active_worker_areas[1], buf);
        }
    }
}

impl ActiveEncoders {
    #[inline]
    pub fn new(
        color: Color,
        workers: u8,
        parent_encoder: Encoder,
        active_scenes: Vec<(u64, SceneEncoder)>,
    ) -> Self {
        Self {
            color,
            workers,
            parent_encoder,
            active_scenes,
        }
    }
}
