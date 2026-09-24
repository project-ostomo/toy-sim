//! Shared dense workspace components. Flex containers wrap only one level.
use crate::{desktop::*, egui};
use egui_flex::{Flex, FlexAlign, FlexItem};

/// OHLC candles and volume from chronological (time, price, quantity) samples.
pub fn price_chart(
    ui: &mut egui::Ui,
    id: &str,
    samples: &[(i64, u64, u64)],
    interval_ms: i64,
    quantity_scale: u64,
    height: f32,
) {
    ui.push_id(id, |ui| {
        if samples.is_empty() {
            empty_state(
                ui,
                "No trade history",
                "Candles appear after the first execution.",
            );
            return;
        }

        let mut candles = std::collections::BTreeMap::new();
        for &(time, price, quantity) in samples {
            let candle = candles
                .entry(time.div_euclid(interval_ms.max(1)))
                .or_insert((price, price, price, price, 0_u64));
            candle.1 = candle.1.max(price);
            candle.2 = candle.2.min(price);
            candle.3 = price;
            candle.4 = candle.4.saturating_add(quantity);
        }

        let low = candles.values().map(|candle| candle.2).min().unwrap() as f64;
        let high = candles.values().map(|candle| candle.1).max().unwrap() as f64;
        let range = (high - low).max(1.);
        let volume = candles
            .values()
            .map(|candle| candle.4)
            .max()
            .unwrap()
            .max(1) as f64;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height.max(160.)),
            egui::Sense::hover(),
        );
        let mut plot = rect.shrink(8.);
        plot.max.x -= 100.;
        plot.max.y -= 22.;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::ZERO, SURFACE_RAISED);

        for step in 0..=4 {
            let y = plot.top() + step as f32 * plot.height() * 0.18;
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                egui::Stroke::new(0.5, MUTED.gamma_multiply(0.3)),
            );
            let price = high - step as f64 * range / 4.;
            painter.text(
                egui::pos2(plot.right() + 8., y),
                egui::Align2::LEFT_CENTER,
                osg_model::economy::format_amount(price.max(0.) as u64),
                egui::FontId::monospace(11.),
                MUTED,
            );
        }

        let first = *candles.first_key_value().unwrap().0;
        let last = *candles.last_key_value().unwrap().0;
        let spacing = plot.width() / (last - first + 1) as f32;
        let y = |price: u64| {
            plot.top() + (1. - (price as f64 - low) / range) as f32 * plot.height() * 0.72
        };
        for (&bucket, &(open, high, low, close, quantity)) in &candles {
            let x = plot.left() + ((bucket - first) as f32 + 0.5) * spacing;
            let color = if close >= open { POSITIVE } else { THREAT };
            let half = (spacing * 0.32).clamp(0.5, 8.);
            painter.line_segment(
                [egui::pos2(x, y(high)), egui::pos2(x, y(low))],
                egui::Stroke::new(1., color),
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - half, y(open.max(close))),
                    egui::pos2(x + half, y(open.min(close)).max(y(open.max(close)) + 1.)),
                ),
                egui::CornerRadius::ZERO,
                color,
            );
            let height = quantity as f64 / volume * plot.height() as f64 * 0.18;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - half, plot.bottom() - height as f32),
                    egui::pos2(x + half, plot.bottom()),
                ),
                egui::CornerRadius::ZERO,
                color.gamma_multiply(0.5),
            );
        }
        let closes: Vec<_> = candles
            .iter()
            .map(|(&bucket, candle)| (bucket, candle.3))
            .collect();
        for (period, color) in [(20, ACCENT), (50, WARNING)] {
            let points: Vec<_> = closes
                .windows(period)
                .map(|window| {
                    let (bucket, _) = window[period - 1];
                    let average = window.iter().map(|(_, price)| *price as u128).sum::<u128>()
                        / period as u128;
                    egui::pos2(
                        plot.left() + ((bucket - first) as f32 + 0.5) * spacing,
                        y(average as u64),
                    )
                })
                .collect();
            if points.len() >= 2 {
                painter.add(egui::Shape::line(points, egui::Stroke::new(1., color)));
            }
        }
        let last_price = candles.last_key_value().unwrap().1.3;
        let last_y = y(last_price);
        painter.line_segment(
            [
                egui::pos2(plot.left(), last_y),
                egui::pos2(plot.right(), last_y),
            ],
            egui::Stroke::new(0.75, MINT.gamma_multiply(0.6)),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(plot.right() + 4., last_y - 8.),
                egui::vec2(96., 16.),
            ),
            0.,
            SURFACE_RAISED,
        );
        painter.text(
            egui::pos2(plot.right() + 8., last_y),
            egui::Align2::LEFT_CENTER,
            osg_model::economy::format_amount(last_price),
            egui::FontId::monospace(11.),
            MINT,
        );
        for (bucket, x, align) in [
            (first, plot.left(), egui::Align2::LEFT_TOP),
            (last, plot.right(), egui::Align2::RIGHT_TOP),
        ] {
            let date = osg_model::calendar::format_utc(bucket * interval_ms.max(1));
            painter.text(
                egui::pos2(x, plot.bottom() + 7.),
                align,
                &date[4..20],
                egui::FontId::monospace(11.),
                MUTED,
            );
        }

        if let Some(pointer) = response
            .hover_pos()
            .filter(|pointer| plot.contains(*pointer))
        {
            let bucket = first + ((pointer.x - plot.left()) / spacing).floor() as i64;
            if let Some(&(open, high, low, close, quantity)) = candles.get(&bucket) {
                painter.line_segment(
                    [
                        egui::pos2(pointer.x, plot.top()),
                        egui::pos2(pointer.x, plot.bottom()),
                    ],
                    egui::Stroke::new(1., ACCENT),
                );
                response.on_hover_ui(|ui| {
                    ui.label(osg_model::calendar::format_utc(bucket * interval_ms.max(1)));
                    for (label, value) in [
                        ("Open", open),
                        ("High", high),
                        ("Low", low),
                        ("Close", close),
                    ] {
                        ui.label(format!(
                            "{label}: {}",
                            osg_model::economy::format_amount(value)
                        ));
                    }
                    let volume = if quantity_scale == osg_model::economy::MONEY_SCALE {
                        osg_model::economy::format_amount(quantity)
                    } else {
                        quantity.to_string()
                    };
                    ui.label(format!("Volume: {volume}"));
                });
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ACCENT, "MA 20");
            ui.colored_label(WARNING, "MA 50");
            ui.weak("UTC · volume below · loaded history · hover for details");
        });
    });
}

pub struct Stat<'a> {
    pub title: &'a str,
    pub value: &'a str,
    pub detail: &'a str,
    pub accent: egui::Color32,
    pub detail_color: egui::Color32,
    pub badge: Option<&'a str>,
}

pub fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1., color))
        .fill(color.linear_multiply(0.08))
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).monospace().size(10.).color(color));
        });
}

pub fn stat_strip(ui: &mut egui::Ui, id: impl egui::AsId, stats: &[Stat<'_>]) {
    ui.push_id(egui::Id::new(id), |ui| {
        ui.set_max_height(360.);
        Flex::new()
            .w_full()
            .wrap(true)
            .align_items(FlexAlign::Stretch)
            .align_items_content(egui::Align2::CENTER_TOP)
            .show(ui, |flex| {
                for stat in stats {
                    let item = FlexItem::default()
                        .grow(1.)
                        .basis(230.)
                        .min_width(180.)
                        .content_id(egui::Id::new((
                            stat.title,
                            stat.value,
                            stat.detail,
                            stat.badge,
                        )))
                        .frame(
                            egui::Frame::new()
                                .fill(SURFACE_RAISED)
                                .stroke(egui::Stroke::new(1., BORDER))
                                .inner_margin(12.),
                        );
                    flex.add_ui(item, |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.colored_label(stat.accent, stat.title);
                            if let Some(label) = stat.badge {
                                badge(ui, label, stat.detail_color);
                            }
                            ui.label(
                                egui::RichText::new(stat.value)
                                    .monospace()
                                    .size(21.)
                                    .color(TEXT),
                            );
                            ui.separator();
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(stat.detail).color(stat.detail_color),
                                )
                                .wrap(),
                            );
                        });
                    });
                }
            });
    });
}

pub fn action_header(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    title: &str,
    subtitle: &str,
    actions: impl FnOnce(&mut egui::Ui),
) {
    ui.push_id(egui::Id::new(id), |ui| {
        ui.set_max_height(90.);
        Flex::new().w_full().wrap(true).show(ui, |flex| {
            flex.add_ui(
                FlexItem::default()
                    .grow(1.)
                    .basis(280.)
                    .content_id(egui::Id::new((title, subtitle))),
                |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.heading(title);
                        ui.colored_label(MUTED, subtitle);
                    });
                },
            );
            flex.add_ui(FlexItem::default(), actions);
        });
    });
}

pub fn filter_bar(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    search: &mut String,
    controls: impl FnOnce(&mut egui::Ui),
) {
    ui.push_id(egui::Id::new(id), |ui| {
        ui.set_max_height(90.);
        Flex::new().w_full().wrap(true).show(ui, |flex| {
            flex.add_ui(
                FlexItem::default()
                    .grow(1.)
                    .shrink()
                    .basis(260.)
                    .min_width(160.),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(search)
                            .hint_text("Search…")
                            .desired_width(f32::INFINITY),
                    );
                },
            );
            flex.add_ui(FlexItem::default(), controls);
        });
    });
}

pub fn flow_row(ui: &mut egui::Ui, id: impl egui::AsId, labels: &[String]) {
    ui.push_id(egui::Id::new(id), |ui| {
        ui.set_max_height(120.);
        Flex::new().w_full().wrap(true).show(ui, |flex| {
            for label in labels {
                flex.add_ui(FlexItem::default().content_id(egui::Id::new(label)), |ui| {
                    ui.label(egui::RichText::new(label).small().color(ACCENT));
                });
            }
        });
    });
}

/// Fixed column geometry is shared by header and virtualized body rows.
pub fn table_row(ui: &mut egui::Ui, widths: &[f32], cells: &[egui::RichText], striped: bool) {
    table_row_aligned(ui, widths, cells, &[], striped);
}

/// Numeric columns can align to the trailing edge while labels retain their
/// common leading edge. Header and body callers use the same column geometry.
pub fn table_row_aligned(
    ui: &mut egui::Ui,
    widths: &[f32],
    cells: &[egui::RichText],
    right_aligned: &[bool],
    striped: bool,
) {
    if striped {
        let rect =
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 24.));
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, SURFACE_RAISED);
    }
    ui.horizontal(|ui| {
        for (index, (width, text)) in widths.iter().zip(cells).enumerate() {
            let layout = if right_aligned.get(index).copied().unwrap_or(false) {
                egui::Layout::right_to_left(egui::Align::Center)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            ui.allocate_ui_with_layout(egui::vec2(*width, 24.), layout, |ui| {
                ui.set_width(*width);
                ui.add(egui::Label::new(text.clone()).truncate());
            });
        }
    });
}

pub fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space(16.);
    ui.label(egui::RichText::new(title).strong());
    ui.colored_label(MUTED, detail);
}
