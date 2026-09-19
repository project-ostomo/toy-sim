use super::*;

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    origin: Option<Id>,
    focus: Option<Id>,
    fit: bool,
) {
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::vec2(0., 220.)),
        egui::Sense::click_and_drag(),
    );
    if state.zoom == 0. || fit {
        state.zoom = (rect.width() / state.cache.bounds.x)
            .min(rect.height() / state.cache.bounds.y)
            .clamp(0.01, 1.5);
        state.pan = egui::Vec2::ZERO;
    }
    if let Some(&index) = focus.and_then(|id| state.cache.systems.get(&id)) {
        state.zoom = state.zoom.max(1.4);
        state.pan = -state.cache.positions[index] * state.zoom;
    }
    if response.dragged() {
        state.pan += ui.input(|input| input.pointer.delta());
    }
    if response.hovered() {
        let old = state.zoom;
        let factor = (ui.input(|input| input.smooth_scroll_delta.y) * 0.003).exp();
        state.zoom = (old * factor).clamp(0.01, 12.);
        if let Some(pointer) = response.hover_pos() {
            let pivot = pointer - rect.center();
            state.pan = pivot - (pivot - state.pan) * state.zoom / old;
        }
    }

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2., egui::Color32::from_rgb(9, 17, 26));
    let positions: Vec<_> = state
        .cache
        .positions
        .iter()
        .map(|position| rect.center() + state.pan + *position * state.zoom)
        .collect();
    let selected = state.selected.or(origin);
    let search = state.search.trim().to_lowercase();
    let matching: Vec<_> = model
        .navigation
        .systems
        .iter()
        .enumerate()
        .map(|(index, system)| {
            state
                .sovereignty
                .is_none_or(|id| system.sovereignty == Some(id))
                && (search.is_empty() || state.cache.names[index].contains(&search))
        })
        .collect();
    let mut highlighted = state.active.systems.clone();
    highlighted.extend(state.suggested.systems.iter().copied());
    for &(a, b, entry, exit) in &state.cache.links {
        let planned = state.active.gates.contains(&entry) || state.active.gates.contains(&exit);
        let suggested =
            state.suggested.gates.contains(&entry) || state.suggested.gates.contains(&exit);
        if planned || suggested {
            highlighted.extend([a, b]);
        }
        let (a_pos, b_pos) = (positions[a], positions[b]);
        if !rect.intersects(egui::Rect::from_two_pos(a_pos, b_pos)) {
            continue;
        }
        let delta = b_pos - a_pos;
        let bend = a_pos
            + egui::vec2(delta.x.signum(), delta.y.signum()) * delta.x.abs().min(delta.y.abs());
        let color = if planned {
            egui::Color32::from_rgb(255, 199, 98)
        } else if suggested {
            ACCENT
        } else if matching[a] && matching[b] {
            egui::Color32::from_rgb(42, 64, 80)
        } else {
            egui::Color32::from_rgb(21, 32, 41)
        };
        let width = if planned {
            2.5
        } else if suggested {
            1.5
        } else {
            0.7
        };
        painter.add(egui::Shape::line(
            vec![a_pos, bend, b_pos],
            egui::Stroke::new(width, color),
        ));
    }
    for &(a, b) in state.active.slips.iter().chain(&state.suggested.slips) {
        let (a, b) = (positions[a], positions[b]);
        if a.distance(b) <= 1. || !rect.intersects(egui::Rect::from_two_pos(a, b)) {
            continue;
        }
        let color = egui::Color32::from_rgb(221, 135, 240);
        let (a, b) = clip_segment(rect, a, b);
        painter.add(egui::Shape::dashed_line(
            &[a, b],
            egui::Stroke::new(2., color),
            7.,
            5.,
        ));
        painter.text(
            a.lerp(b, 0.5),
            egui::Align2::CENTER_BOTTOM,
            "SLIP",
            egui::FontId::monospace(10.),
            color,
        );
    }

    let visible: Vec<_> = positions
        .iter()
        .enumerate()
        .filter_map(|(index, &position)| rect.expand(10.).contains(position).then_some(index))
        .collect();
    let hovered = response.hover_pos().and_then(|pointer| {
        visible
            .iter()
            .copied()
            .filter(|&index| matching[index] && positions[index].distance_sq(pointer) <= 81.)
            .min_by(|&a, &b| {
                positions[a]
                    .distance_sq(pointer)
                    .total_cmp(&positions[b].distance_sq(pointer))
            })
    });
    if response.clicked() {
        if let Some(index) = hovered {
            state.selected = Some(model.navigation.systems[index].id);
        }
    }
    if let Some(index) = hovered {
        response.on_hover_ui_at_pointer(|ui| {
            let system = &model.navigation.systems[index];
            ui.strong(&system.name);
            if let Some(sovereignty) = system
                .sovereignty
                .and_then(|id| model.society.directory.sovereignties.get(&id))
            {
                ui.label(&sovereignty.name);
            }
            ui.weak(format!(
                "Population {} · Click to select",
                population(system.population)
            ));
        });
    }
    let radius = (3.5 * state.zoom.sqrt()).clamp(1.4, 6.);
    let mut labels = Vec::new();
    for index in visible {
        let system = &model.navigation.systems[index];
        let position = positions[index];
        let own = origin == Some(system.id);
        let chosen = selected == Some(system.id);
        let route = highlighted.contains(&index);
        let hovered = hovered == Some(index);
        let mut color = polity_color(
            system
                .sovereignty
                .and_then(|id| model.society.directory.sovereignties.get(&id))
                .map(|s| s.bloc),
        );
        if !matching[index] && !own && !chosen && !route {
            color = color.gamma_multiply(0.25);
        }
        painter.circle_filled(position, radius, color);
        if own {
            painter.circle_stroke(
                position,
                radius + 3.,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
        }
        if chosen {
            painter.circle_stroke(position, radius + 6., egui::Stroke::new(1.5, ACCENT));
        }
        if own
            || chosen
            || hovered
            || (state.zoom >= 1.1 && matching[index])
            || (route && state.zoom >= 0.65)
        {
            labels.push((!(own || chosen || hovered), !route, index, color));
        }
    }
    labels.sort_unstable_by_key(|&(ordinary, off_route, index, _)| (ordinary, off_route, index));
    let mut occupied = BTreeSet::new();
    for (ordinary, _, index, color) in labels.into_iter().take(128) {
        let system = &model.navigation.systems[index];
        let color = if ordinary {
            color
        } else {
            egui::Color32::WHITE
        };
        let galley =
            painter.layout_no_wrap(system.name.clone(), egui::FontId::proportional(11.), color);
        let position = positions[index] + egui::vec2(-galley.size().x * 0.5, radius + 5.);
        let bounds = egui::Rect::from_min_size(position, galley.size()).expand(3.);
        let cells = label_cells(bounds);
        if ordinary && cells.iter().any(|cell| occupied.contains(cell)) {
            continue;
        }
        occupied.extend(cells);
        painter.galley(position, galley, color);
    }
}

pub(super) fn legend(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        for (bloc, label) in [
            (Bloc::Union, "USE"),
            (Bloc::League, "LFS member states"),
            (Bloc::NonAligned, "Non-aligned"),
        ] {
            ui.colored_label(polity_color(Some(bloc)), format!("● {label}"));
        }
        ui.colored_label(egui::Color32::from_rgb(255, 199, 98), "Queued route");
        ui.colored_label(egui::Color32::from_rgb(221, 135, 240), "Slip");
        ui.weak("Drag · scroll to zoom");
    });
}

fn label_cells(rect: egui::Rect) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();
    for x in (rect.min.x / 32.).floor() as i32..=(rect.max.x / 32.).floor() as i32 {
        for y in (rect.min.y / 16.).floor() as i32..=(rect.max.y / 16.).floor() as i32 {
            cells.push((x, y));
        }
    }
    cells
}

fn clip_segment(rect: egui::Rect, a: egui::Pos2, b: egui::Pos2) -> (egui::Pos2, egui::Pos2) {
    let direction = b - a;
    let mut lo: f32 = 0.;
    let mut hi: f32 = 1.;
    for (start, delta, min, max) in [
        (a.x, direction.x, rect.min.x, rect.max.x),
        (a.y, direction.y, rect.min.y, rect.max.y),
    ] {
        if delta.abs() > f32::EPSILON {
            let p = (min - start) / delta;
            let q = (max - start) / delta;
            lo = lo.max(p.min(q));
            hi = hi.min(p.max(q));
        }
    }
    (a + direction * lo, a + direction * hi.max(lo))
}
