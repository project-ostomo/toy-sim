//! Gallery framing uses the same launcher, status bar and windows as the client.
use super::*;

#[derive(Default)]
pub struct Workspace {
    shell: Shell,
    panes: TestPanes,
    foreground: Desktop,
    violations: Vec<String>,
}

impl Workspace {
    pub fn set_loading(&mut self, spec: WindowSpec, loading: bool) {
        self.foreground.set_loading(spec, loading);
    }

    pub fn new(ctx: &egui::Context) -> Self {
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        Self::default()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        spec: WindowSpec,
        model: &FrameModel,
        contents: impl FnOnce(&mut egui::Ui),
    ) {
        let ctx = ui.ctx();
        ui.painter()
            .rect_filled(ctx.content_rect(), 0., egui::Color32::from_rgb(7, 12, 18));
        let selection = Selection {
            ship: model.ship.map(|ship| ship.ship),
            target: model.rows.first().map(|row| row.target),
            ..Default::default()
        };
        let mut intents = Vec::new();
        panels::draw(
            ctx,
            false,
            &mut self.shell,
            &mut self.panes.get(),
            model,
            &selection,
            &[],
            &ChatState::default(),
            &QueryState::Loading,
            &QueryState::Loading,
            &QueryState::Loading,
            &mut intents,
        );
        assert!(intents.is_empty());
        self.foreground.open(spec);
        self.foreground.show(ctx, spec, |ui| {
            ctx.move_to_top(ui.layer_id());
            // Capture the allocation before widgets can expand the UI. A
            // scroll area's content may be larger; its allocated viewport may not.
            let allocated = ui.max_rect();
            contents(ui);
            let content = ui.min_rect();
            if !allocated.expand(1.).contains_rect(content) {
                self.violations.push(format!(
                    "{} content exceeds window allocation: content {content:?}, allocation {allocated:?}",
                    spec.id,
                ));
            }
            let viewport = ctx.content_rect();
            if content.right() > viewport.right() + 1.
                || content.bottom() > viewport.bottom() + 1.
            {
                self.violations.push(format!(
                    "{} content exceeds viewport: {content:?}",
                    spec.id,
                ));
            }
        });
    }

    /// Call after processing textures and saving the final screenshot. Panicking
    /// during egui rendering would drop pending texture deltas while unwinding.
    pub fn assert_bounds(&self) {
        assert!(self.violations.is_empty(), "{}", self.violations.join("\n"));
    }
}
