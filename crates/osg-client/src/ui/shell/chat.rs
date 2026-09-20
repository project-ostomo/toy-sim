mod layout;

use super::*;
use osg_model::chat::{ChatMessage, MAX_MESSAGE_BYTES, valid_text};

#[derive(Default)]
pub(super) struct State {
    draft: String,
    layout: layout::Layout,
    generation: u64,
    scroll_offset: f32,
    at_bottom: bool,
    pending: Option<(Id, String)>,
    refocus: bool,
    error: Option<String>,
}

impl State {
    pub fn sent(&mut self, id: Id, text: String) {
        self.pending = Some((id, text));
        self.error = None;
    }

    pub fn receive(&mut self, results: &[CommandResult]) {
        let Some((pending, _)) = &self.pending else {
            return;
        };
        let Some(result) = results.iter().find(|result| result.id == *pending) else {
            return;
        };

        let (_, sent) = self.pending.take().unwrap();
        self.refocus = true;
        self.error = result.error.clone();
        if self.error.is_none() && self.draft == sent {
            self.draft.clear();
        }
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    chat: &ChatState,
    intents: &mut Vec<Intent>,
) {
    if state.generation != chat.generation {
        state.layout = layout::Layout::default();
        state.generation = chat.generation;
        state.scroll_offset = 0.0;
        state.at_bottom = true;
    }

    ui.horizontal(|ui| {
        ui.label(Icon::Broadcast.text(18.0).color(ACCENT));
        ui.strong("LOCAL · 500 AU");
        if let Some(ship) = model.ship {
            ui.weak(ship_name(ship));
        }
    });
    if chat.interrupted {
        ui.weak(
            "Earlier history retained; messages while this window was closed are not replayed.",
        );
    }
    if chat.missed > 0 {
        ui.colored_label(
            MUTED,
            format!("{} earlier messages are unavailable", chat.missed),
        );
    }

    egui::Panel::bottom(ui.id().with("local_composer"))
        .frame(egui::Frame::NONE)
        .show_separator_line(false)
        .show(ui, |ui| {
            composer(ui, state, model.connected && chat.can_send(), intents)
        });

    if chat.unavailable {
        ui.weak("Local communications unavailable from this ship's current location.");
    } else if !chat.can_send() {
        ui.weak("Connecting to the focused ship's local channel…");
    }

    let font = egui::TextStyle::Body.resolve(ui.style());
    let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(1.0);
    let rebuilt = state.layout.prepare(ui, width, &font);
    let removed = state
        .layout
        .retain_from(chat.messages.front().map(|message| message.sequence));
    let last = state.layout.last_sequence();
    let first_new = chat
        .messages
        .partition_point(|message| last.is_some_and(|last| message.sequence <= last));
    for message in chat.messages.iter().skip(first_new) {
        let heading = heading(message, &model.society.directory);
        state
            .layout
            .append(ui, message.sequence, &heading, &message.text);
    }

    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("local_scrollback", chat.generation))
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .max_height(ui.available_height().max(0.0))
            .stick_to_bottom(true);
        if removed > 0 && !state.at_bottom && !rebuilt {
            area = area.vertical_scroll_offset(
                (state.scroll_offset - removed as f32 * row_height).max(0.0),
            );
        }

        let output = area.show_rows(ui, row_height, state.layout.lines.len(), |ui, rows| {
            for index in rows {
                let line = &state.layout.lines[index];
                let message = chat
                    .messages
                    .binary_search_by_key(&line.sequence, |message| message.sequence)
                    .ok()
                    .map(|index| &chat.messages[index]);
                let color = if line.heading {
                    message.map_or(MUTED, |message| sender_color(message, model.society))
                } else {
                    TEXT
                };
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
                ui.painter().text(
                    rect.left_top(),
                    egui::Align2::LEFT_TOP,
                    &line.text,
                    font.clone(),
                    color,
                );
                if let Some(message) = message {
                    response
                        .on_hover_text(osg_model::calendar::format_utc(message.calendar_unix_ms));
                }
            }
        });
        state.scroll_offset = output.state.offset.y;
        state.at_bottom =
            output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 1.0;
    });
}

fn composer(ui: &mut egui::Ui, state: &mut State, available: bool, intents: &mut Vec<Intent>) {
    ui.separator();
    let enabled = available && state.pending.is_none();
    let response = ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(&mut state.draft)
            .hint_text("Message local…")
            .char_limit(MAX_MESSAGE_BYTES)
            .desired_width(f32::INFINITY),
    );
    if enabled && std::mem::take(&mut state.refocus) {
        response.request_focus();
    }
    let entered = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    let valid = valid_text(&state.draft);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(enabled && valid, egui::Button::new("Send"))
            .clicked();
        ui.colored_label(
            if state.draft.len() > MAX_MESSAGE_BYTES {
                THREAT
            } else {
                MUTED
            },
            format!("{} / {} bytes", state.draft.len(), MAX_MESSAGE_BYTES),
        );
        if state.pending.is_some() {
            ui.weak("Sending…");
        }
        if enabled && valid && (clicked || entered) {
            intents.push(Intent::Chat(state.draft.clone()));
            response.request_focus();
        }
    });
    if let Some(error) = &state.error {
        ui.colored_label(THREAT, error);
    }
}

fn heading(message: &ChatMessage, directory: &ownership::OwnershipDirectory) -> String {
    let timestamp = osg_model::calendar::format_utc(message.calendar_unix_ms);
    let mut parts = timestamp.split_whitespace();
    parts.next();
    let date = parts.next().unwrap_or("---- -- --");
    let time = parts.next().unwrap_or("--:--:--");
    let mut heading = format!("[{date} {time}] {}", message.sender_name);
    if let Some(organization) = message.advertised_organization {
        let organization = directory
            .organizations
            .get(&organization)
            .map_or("Unknown organization", |organization| {
                organization.name.as_str()
            });
        heading.push_str(" · ");
        heading.push_str(organization);
    }
    heading
}

fn sender_color(message: &ChatMessage, society: &ownership::SocietySnapshot) -> egui::Color32 {
    super::super::standing::color(society.directory.advertised_standing(
        society.account,
        message.advertised_owner,
        message.advertised_organization,
    ))
}

#[cfg(test)]
mod tests;
