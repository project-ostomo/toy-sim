use super::super::egui;
use std::collections::VecDeque;

pub(super) struct Line {
    pub sequence: u64,
    pub heading: bool,
    pub text: String,
}

#[derive(Default)]
pub(super) struct Layout {
    width: u32,
    pixels_per_point: u32,
    font: Option<egui::FontId>,
    messages: VecDeque<(u64, usize)>,
    pub lines: VecDeque<Line>,
}

impl Layout {
    pub fn prepare(&mut self, ui: &egui::Ui, width: f32, font: &egui::FontId) -> bool {
        let width = width.floor().max(1.0) as u32;
        let pixels_per_point = ui.ctx().pixels_per_point().to_bits();
        if self.width == width
            && self.pixels_per_point == pixels_per_point
            && self.font.as_ref() == Some(font)
        {
            return false;
        }

        self.width = width;
        self.pixels_per_point = pixels_per_point;
        self.font = Some(font.clone());
        self.messages.clear();
        self.lines.clear();
        true
    }

    pub fn retain_from(&mut self, first_sequence: Option<u64>) -> usize {
        let mut removed = 0;
        while self
            .messages
            .front()
            .is_some_and(|(sequence, _)| first_sequence.is_none_or(|first| *sequence < first))
        {
            let (_, count) = self.messages.pop_front().unwrap();
            self.lines.drain(..count);
            removed += count;
        }
        removed
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.messages.back().map(|(sequence, _)| *sequence)
    }

    pub fn append(&mut self, ui: &egui::Ui, sequence: u64, heading: &str, body: &str) {
        let before = self.lines.len();
        let font = self.font.as_ref().unwrap();
        for (text, heading) in [(heading, true), (body, false)] {
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout(
                    text.into(),
                    font.clone(),
                    egui::Color32::WHITE,
                    self.width as f32,
                )
            });
            for row in &galley.rows {
                self.lines.push_back(Line {
                    sequence,
                    heading,
                    text: row.text(),
                });
            }
        }
        self.messages
            .push_back((sequence, self.lines.len() - before));
    }
}
