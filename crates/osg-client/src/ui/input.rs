use bevy::prelude::*;
use osg_ui::{
    bevy_egui::{EguiContexts, EguiPreUpdateSet, EguiPrimaryContextPass},
    egui,
};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameplayInput {
    Mouse,
    Keyboard,
}

#[derive(Resource, Default)]
pub struct InputCapture {
    pub mouse_available: bool,
    keyboard_available: bool,
}

pub fn install(app: &mut App) {
    app.init_resource::<InputCapture>();
    app.add_systems(PreUpdate, capture.after(EguiPreUpdateSet::BeginPass));
    app.add_systems(
        EguiPrimaryContextPass,
        capture.after(super::shell::ShellDraw),
    );
    app.configure_sets(
        Update,
        (
            GameplayInput::Mouse
                .run_if(mouse_available)
                .in_set(crate::state::ClientSystems::Gameplay),
            GameplayInput::Keyboard
                .run_if(keyboard_available)
                .in_set(crate::state::ClientSystems::Gameplay),
        ),
    );
    app.configure_sets(
        EguiPrimaryContextPass,
        (
            GameplayInput::Mouse
                .after(capture)
                .run_if(mouse_available)
                .in_set(crate::state::ClientSystems::Gameplay),
            GameplayInput::Keyboard
                .after(capture)
                .run_if(keyboard_available)
                .in_set(crate::state::ClientSystems::Gameplay),
        ),
    );
}

pub fn pointer_available(ctx: &egui::Context) -> bool {
    !ctx.is_pointer_over_egui()
        && !ctx.egui_wants_pointer_input()
        && !ctx.egui_is_using_pointer()
        && !ctx.any_popup_open()
        && !ctx
            .input(|input| input.pointer.interact_pos())
            .and_then(|position| ctx.layer_id_at(position))
            .is_some_and(|layer| layer == super::console::layer())
}

fn capture(mut contexts: EguiContexts, mut capture: ResMut<InputCapture>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        *capture = InputCapture::default();
        return;
    };
    capture.mouse_available = pointer_available(ctx);
    capture.keyboard_available = !ctx.egui_wants_keyboard_input() && !ctx.any_popup_open();
}

fn mouse_available(capture: Res<InputCapture>) -> bool {
    capture.mouse_available
}

fn keyboard_available(capture: Res<InputCapture>) -> bool {
    capture.keyboard_available
}
