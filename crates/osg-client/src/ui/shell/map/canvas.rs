use super::*;

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    origin: Option<Id>,
    focus: Option<Id>,
    fit: bool,
    fit_route: bool,
) {
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::vec2(0., 220.)),
        egui::Sense::click_and_drag(),
    );
    if state.camera.scale == 0. || fit {
        state.camera.fit(
            state
                .browser_systems
                .iter()
                .map(|&index| state.cache.positions[index]),
            rect,
        );
    }
    if fit_route {
        let route = if state.route.plan().is_some() {
            &state.suggested
        } else {
            &state.active
        };
        state.camera.fit(
            route
                .systems
                .iter()
                .map(|&index| state.cache.positions[index]),
            rect,
        );
    }
    if let Some(&index) = focus.and_then(|id| state.cache.systems.get(&id)) {
        state.camera.focus(state.cache.positions[index]);
    }
    state.camera.update(ui, &response);

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2., egui::Color32::from_rgb(9, 17, 26));
    let selected = state.selected.or(origin);
    let mut highlighted = state.active.systems.clone();
    highlighted.extend(state.suggested.systems.iter().copied());
    let tree = &state.cache.inhabited_tree;
    let mut samples = tree.visible(&state.cache.positions, &state.camera, rect);
    samples.extend(highlighted.iter().copied());
    samples.extend(
        state
            .active
            .stops
            .iter()
            .chain(&state.suggested.stops)
            .map(|&(_, index)| index),
    );
    samples.extend(
        selected
            .and_then(|id| state.cache.systems.get(&id))
            .copied(),
    );
    samples.extend(origin.and_then(|id| state.cache.systems.get(&id)).copied());
    let projected: std::collections::BTreeMap<_, _> = samples
        .into_iter()
        .map(|index| {
            (
                index,
                state.camera.project(state.cache.positions[index], rect),
            )
        })
        .collect();
    let positions: std::collections::BTreeMap<_, _> = projected
        .iter()
        .map(|(&index, &(position, _))| (index, position))
        .collect();
    reference_plane(&painter, rect, state);
    let search = state.search.trim().to_lowercase();
    let matching: std::collections::BTreeMap<_, _> = positions
        .keys()
        .map(|&index| {
            let system = &model.navigation.systems[index];
            (
                index,
                state
                    .sovereignty
                    .is_none_or(|id| system.sovereignty == Some(id))
                    && (search.is_empty() || state.cache.names[index].contains(&search))
                    && state.browser_systems.contains(&index),
            )
        })
        .collect();
    for &(a, b, speed_ly_s, loss) in state.active.slips.iter().chain(&state.suggested.slips) {
        let (a, b) = (positions[&a], positions[&b]);
        if a.distance(b) <= 1. || !rect.intersects(egui::Rect::from_two_pos(a, b)) {
            continue;
        }
        let color = crate::ui::travel_risk::color(loss);
        let speed_c = speed_ly_s * travel::slip::LY_M / 299_792_458.0;
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
            loss.map_or_else(
                || format!("SLIP · {speed_c:.1} c · risk unknown"),
                |loss| format!("SLIP · {speed_c:.1} c · {loss:.2} ppm"),
            ),
            egui::FontId::monospace(10.),
            color,
        );
    }

    let mut visible: Vec<_> = positions
        .iter()
        .filter_map(|(&index, &position)| rect.expand(10.).contains(position).then_some(index))
        .collect();
    let hovered = response.hover_pos().and_then(|pointer| {
        visible
            .iter()
            .copied()
            .filter(|&index| {
                (matching[&index]
                    || highlighted.contains(&index)
                    || selected == Some(model.navigation.systems[index].id))
                    && positions[&index].distance_sq(pointer) <= 81.
            })
            .min_by(|&a, &b| {
                positions[&a]
                    .distance_sq(pointer)
                    .total_cmp(&positions[&b].distance_sq(pointer))
                    .then_with(|| projected[&b].1.total_cmp(&projected[&a].1))
            })
    });
    if response.clicked() {
        if let Some(index) = hovered {
            state.selected = Some(model.navigation.systems[index].id);
        }
    }
    if response.double_clicked() {
        if let Some(index) = hovered {
            state.camera.focus(state.cache.positions[index]);
        }
    }
    if let Some(index) = hovered {
        response.on_hover_ui_at_pointer(|ui| {
            let system = &model.navigation.systems[index];
            ui.strong(&system.name);
            if model.inhabited.systems.binary_search(&system.id).is_ok() {
                ui.label("Inhabited · public directory transmitter");
            }
            if let Some(sovereignty) = system
                .sovereignty
                .and_then(|id| model.inhabited.sovereignties.get(&id))
            {
                ui.label(&sovereignty.name);
            }
            ui.weak("Click to select · Double-click to focus");
        });
    }
    let radius = (2.0 + state.camera.scale.sqrt() as f32 * 0.25).clamp(2., 5.);
    visible.sort_unstable_by(|&a, &b| projected[&a].1.total_cmp(&projected[&b].1));
    let depth_span = state.cache.bounds.length().max(1.);
    let mut labels = Vec::new();
    for index in visible {
        let system = &model.navigation.systems[index];
        let position = positions[&index];
        let own = origin == Some(system.id);
        let chosen = selected == Some(system.id);
        let route = highlighted.contains(&index);
        let hovered = hovered == Some(index);
        let mut color = polity_color(
            system
                .sovereignty
                .and_then(|id| model.inhabited.sovereignties.get(&id))
                .map(|s| s.bloc),
        );
        if !matching[&index] && !own && !chosen && !route {
            color = color.gamma_multiply(0.25);
        } else if !own && !chosen && !route && !hovered {
            let depth = (projected[&index].1 / depth_span + 0.5).clamp(0., 1.);
            color = color.gamma_multiply((0.45 + 0.55 * depth) as f32);
        }
        painter.circle_filled(position, radius, color);
        if model.inhabited.systems.binary_search(&system.id).is_ok() {
            painter.circle_stroke(position, radius + 1.5, egui::Stroke::new(1., color));
        }
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
        if own || chosen || hovered || (state.camera.scale >= 20. && matching[&index]) || route {
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
        let position = positions[&index] + egui::vec2(-galley.size().x * 0.5, radius + 5.);
        let bounds = egui::Rect::from_min_size(position, galley.size()).expand(3.);
        let cells = label_cells(bounds);
        if ordinary && cells.iter().any(|cell| occupied.contains(cell)) {
            continue;
        }
        occupied.extend(cells);
        painter.galley(position, galley, color);
    }
    let route = if state.route.plan().is_some() {
        &state.suggested
    } else {
        &state.active
    };
    for &(number, index) in &route.stops {
        let position = positions[&index];
        if rect.contains(position) {
            painter.text(
                position + egui::vec2(radius + 5., -radius - 3.),
                egui::Align2::LEFT_BOTTOM,
                number.to_string(),
                egui::FontId::monospace(11.),
                ACCENT,
            );
        }
    }
    let distance = 10_f64.powf((90. / state.camera.scale).log10().floor());
    let start = rect.left_bottom() + egui::vec2(14., -16.);
    let end = start + egui::vec2((distance * state.camera.scale) as f32, 0.);
    painter.line_segment([start, end], egui::Stroke::new(1., MUTED));
    painter.text(
        start - egui::vec2(0., 4.),
        egui::Align2::LEFT_BOTTOM,
        format!("{distance} ly"),
        egui::FontId::monospace(10.),
        MUTED,
    );
}

fn reference_plane(painter: &egui::Painter, rect: egui::Rect, state: &State) {
    let extent = state.cache.bounds.max_element().max(10.);
    let step = 10_f64.powf((extent / 10.).log10().floor());
    let limit = (extent * 0.5 / step).ceil() as i32;
    let edge = limit as f64 * step;
    for index in -limit..=limit {
        let offset = index as f64 * step;
        for (a, b) in [
            (
                glam::DVec3::new(-edge, offset, 0.),
                glam::DVec3::new(edge, offset, 0.),
            ),
            (
                glam::DVec3::new(offset, -edge, 0.),
                glam::DVec3::new(offset, edge, 0.),
            ),
        ] {
            let a = state.camera.project(a, rect).0;
            let b = state.camera.project(b, rect).0;
            painter.line_segment(
                [a, b],
                egui::Stroke::new(0.5, egui::Color32::from_rgb(17, 29, 39)),
            );
        }
    }
    if let Some(&index) = state.selected.and_then(|id| state.cache.systems.get(&id)) {
        let point = state.cache.positions[index];
        let foot = glam::DVec3::new(point.x, point.y, 0.);
        let a = state.camera.project(point, rect).0;
        let b = state.camera.project(foot, rect).0;
        if rect.intersects(egui::Rect::from_two_pos(a, b)) {
            let (a, b) = clip_segment(rect, a, b);
            painter.add(egui::Shape::dashed_line(
                &[a, b],
                egui::Stroke::new(1., MUTED),
                3.,
                4.,
            ));
        }
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
        ui.weak("◎ Inhabited");
        ui.weak("Right-drag rotate · Middle-drag / Shift+right-drag pan · Scroll zoom · Double-click focus");
    });
    ui.horizontal_wrapped(|ui| {
        ui.weak("Leg failure risk:");
        for (loss, label) in [
            (100.0, "≤100 ppm"),
            (1_000.0, "≤1,000 ppm"),
            (10_000.0, "≤10,000 ppm"),
            (1_000_000.0, ">10,000 ppm"),
        ] {
            ui.colored_label(crate::ui::travel_risk::color(Some(loss)), label);
        }
        ui.colored_label(crate::ui::travel_risk::color(None), "Unknown");
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
