//! Standalone programmable 2D surfaces. They cannot embed native instruments.
use bevy_egui::egui;
use osg_model::drawing::ScreenImage;
use osg_ship_api::abi::{ScreenDefinition, ScreenEvent, Text64};

pub fn show_remote(
    ui: &mut egui::Ui,
    definition: &osg_model::presentation::ScreenDefinition,
    frame: Option<&ScreenImage>,
) -> Vec<ScreenEvent> {
    let definition = ScreenDefinition {
        id: u64::from(definition.slot),
        width: u64::from(definition.width.max(1)),
        height: u64::from(definition.height.max(1)),
        title: Text64::new(&definition.title),
    };

    show(ui, &definition, frame)
}

pub fn show(
    ui: &mut egui::Ui,
    definition: &ScreenDefinition,
    frame: Option<&ScreenImage>,
) -> Vec<ScreenEvent> {
    let width = ui.available_width().clamp(128., 1024.);
    let height = width * definition.height as f32 / definition.width as f32;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if let Some(frame) = frame {
        crate::mfd::paint(ui.painter(), rect, frame);
    } else {
        ui.painter().rect_filled(rect, 0., egui::Color32::BLACK);
    }
    let mut output = vec![];
    let origin = |p: egui::Pos2| {
        [
            (p.x - rect.left()) as f64 / width as f64 * definition.width as f64,
            (p.y - rect.top()) as f64 / height as f64 * definition.height as f64,
        ]
    };
    let modifiers = |m: egui::Modifiers| {
        u64::from(m.shift)
            | u64::from(m.ctrl) << 1
            | u64::from(m.alt) << 2
            | u64::from(m.command) << 3
    };
    let hovered = response.contains_pointer();
    let captured = response.is_pointer_button_down_on();
    let focused = response.has_focus();
    let dragging = response.dragged();
    let drag_stopped = response.drag_stopped();
    ui.input_mut(|input| {
        input.events.retain(|event| {
            let mut e = ScreenEvent {
                screen: definition.id,
                ..Default::default()
            };
            match event {
                egui::Event::PointerMoved(p) if (hovered && rect.contains(*p)) || dragging => {
                    e.kind = 0;
                    [e.x, e.y] = origin(*p);
                    output.push(e);
                    return true;
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: m,
                } if (hovered && rect.contains(*pos)) || captured || dragging || drag_stopped => {
                    e.kind = if *pressed { 1 } else { 2 };
                    [e.x, e.y] = origin(*pos);
                    e.code = match button {
                        egui::PointerButton::Primary => 0,
                        egui::PointerButton::Secondary => 1,
                        _ => 2,
                    };
                    e.modifiers = modifiers(*m);
                    output.push(e);
                    return true;
                }
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers: m,
                    ..
                } if focused => {
                    e.kind = if *pressed { 3 } else { 5 };
                    e.code = match key {
                        egui::Key::ArrowUp => 0x110000,
                        egui::Key::ArrowDown => 0x110001,
                        egui::Key::ArrowLeft => 0x110002,
                        egui::Key::ArrowRight => 0x110003,
                        egui::Key::Home => 0x110004,
                        egui::Key::End => 0x110005,
                        egui::Key::PageUp => 0x110006,
                        egui::Key::PageDown => 0x110007,
                        egui::Key::Space => 32,
                        egui::Key::Enter => 13,
                        egui::Key::Escape => 27,
                        egui::Key::Tab => 9,
                        egui::Key::Backspace => 8,
                        egui::Key::Delete => 127,
                        _ => {
                            let name = key.name();
                            if name.len() == 1 {
                                name.as_bytes()[0] as u64
                            } else {
                                return true;
                            }
                        }
                    };
                    e.modifiers = modifiers(*m);
                    output.push(e);
                    return false;
                }
                egui::Event::Text(text) if focused => {
                    e.kind = 4;
                    let mut text = text.as_str();
                    while !text.is_empty() && output.len() < 128 {
                        e.text = Text64::new(text);
                        text = &text[e.text.len as usize..];
                        output.push(e);
                    }
                    return false;
                }
                _ => {}
            }
            true
        });
    });
    output.truncate(128);
    output
}
