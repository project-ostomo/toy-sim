use super::selection::Selection;
use crate::state::{GameSession, Outgoing, OwnedShip, ScreenFrames, SessionReset, ShipDetails};
use bevy::prelude::*;
use osg_model::{Action, Id, ShipCommand};
use osg_ui::{
    bevy_egui::{EguiContexts, EguiPrimaryContextPass},
    egui,
};

#[derive(Resource)]
pub struct Mfd {
    open: bool,
    slot: u8,
    subscribed: Option<(Id, u8)>,
}

impl Default for Mfd {
    fn default() -> Self {
        Self {
            open: false,
            slot: 0,
            subscribed: None,
        }
    }
}

impl Mfd {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn subscribe(
        &mut self,
        desired: Option<(Id, u8)>,
        outgoing: &mut Outgoing,
        frames: &mut ScreenFrames,
    ) {
        if self.subscribed == desired {
            return;
        }
        if let Some((ship, slot)) = self.subscribed.take() {
            outgoing.push(Action::ScreenUnsubscribe { ship, slot });
            frames.0.remove(&(ship, slot));
        }
        if let Some((ship, slot)) = desired {
            frames.0.remove(&(ship, slot));
            outgoing.push(Action::ScreenSubscribe { ship, slot, hz: 10 });
        }
        self.subscribed = desired;
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<Mfd>()
        .add_observer(|_: On<SessionReset>, mut mfd: ResMut<Mfd>| {
            mfd.subscribed = None;
        })
        .add_systems(
            EguiPrimaryContextPass,
            draw.after(super::shell::ShellDraw)
                .in_set(crate::state::ClientSystems::Gameplay),
        );
}

fn draw(
    mut contexts: EguiContexts,
    mut mfd: ResMut<Mfd>,
    selection: Res<Selection>,
    _session: Res<GameSession>,
    mut frames: ResMut<ScreenFrames>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let selected = ships
        .iter()
        .find(|(ship, _)| Some(ship.0.ship) == selection.ship);
    let connected = true;
    let mut open = mfd.open;
    if open {
        egui::Window::new("FIRMWARE MFD")
            .id(egui::Id::new("firmware_mfd"))
            .open(&mut open)
            .default_width(780.)
            .vscroll(true)
            .show(ctx, |ui| {
                let Some((ship, details)) = selected else {
                    ui.weak("Select a ship to view its computer screens.");
                    return;
                };
                let definitions = details
                    .map(|details| details.0.screens.as_slice())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Screen");
                    egui::ComboBox::from_id_salt("mfd_slot")
                        .selected_text(format!("{}", mfd.slot + 1))
                        .show_ui(ui, |ui| {
                            for slot in 0..osg_model::drawing::MAX_SCREENS as u8 {
                                let title = definitions
                                    .iter()
                                    .find(|d| d.slot == slot)
                                    .map(|d| d.title.as_str());
                                let label = title.map_or_else(
                                    || format!("{}", slot + 1),
                                    |title| format!("{} · {title}", slot + 1),
                                );
                                ui.selectable_value(&mut mfd.slot, slot, label);
                            }
                        });
                });
                let key = (ship.0.ship, mfd.slot);
                let update = frames.0.get(&key).filter(|_| mfd.subscribed == Some(key));
                if let Some(error) = update.and_then(|update| update.error.as_deref()) {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let Some((update, frame)) =
                    update.and_then(|update| update.frame.as_ref().map(|frame| (update, frame)))
                else {
                    ui.weak(if connected {
                        "Waiting for the computer to draw this screen…"
                    } else {
                        "Disconnected"
                    });
                    return;
                };
                let definition = osg_model::presentation::ScreenDefinition {
                    slot: mfd.slot,
                    width: frame.width,
                    height: frame.height,
                    title: definitions
                        .iter()
                        .find(|d| d.slot == mfd.slot)
                        .map(|d| d.title.clone())
                        .unwrap_or_default(),
                };
                if !definition.title.is_empty() {
                    ui.label(&definition.title);
                }
                ui.add_enabled_ui(connected, |ui| {
                    for event in osg_ui::screens::show_remote(ui, &definition, Some(frame)) {
                        outgoing.ship(
                            &ship.0,
                            ShipCommand::ScreenInput {
                                slot: mfd.slot,
                                revision: update.revision,
                                kind: event.kind as u8,
                                code: event.code,
                                modifiers: event.modifiers,
                                xy: [event.x, event.y],
                                text: event.text.as_str().unwrap_or_default().to_owned(),
                            },
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (index, label) in frame.buttons.iter().enumerate() {
                            if let Some(label) = label {
                                if ui.button(label).clicked() {
                                    outgoing.ship(
                                        &ship.0,
                                        ShipCommand::ScreenInput {
                                            slot: mfd.slot,
                                            revision: update.revision,
                                            kind: osg_ui::screens::BEZEL_EVENT,
                                            code: index as u64,
                                            modifiers: 0,
                                            xy: [0., 0.],
                                            text: String::new(),
                                        },
                                    );
                                }
                            }
                        }
                    });
                });
            });
    }
    mfd.open = open;
    let desired = selected
        .filter(|_| open && connected)
        .map(|(ship, _)| (ship.0.ship, mfd.slot));
    mfd.subscribe(desired, &mut outgoing, &mut frames);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_ship_or_screen_and_closing_updates_subscriptions() {
        let mut mfd = Mfd::default();
        let mut outgoing = Outgoing::default();
        let mut frames = ScreenFrames::default();
        let first = Id([1; 16]);
        let second = Id([2; 16]);
        mfd.subscribe(Some((first, 1)), &mut outgoing, &mut frames);
        mfd.subscribe(Some((first, 1)), &mut outgoing, &mut frames);
        mfd.subscribe(Some((second, 0)), &mut outgoing, &mut frames);
        mfd.subscribe(None, &mut outgoing, &mut frames);
        assert!(matches!(outgoing.pending(), [
            (_, Action::ScreenSubscribe { ship: a, slot: 1, .. }),
            (_, Action::ScreenUnsubscribe { ship: b, slot: 1 }),
            (_, Action::ScreenSubscribe { ship: c, slot: 0, .. }),
            (_, Action::ScreenUnsubscribe { ship: d, slot: 0 }),
        ] if *a == first && *b == first && *c == second && *d == second));
    }
}
