use super::*;
use economy::{format_amount, parse_amount};
use osg_model::market::{Instrument, MarketCommand, Side};
use osg_ui::components;
use ownership::Principal;

#[derive(Default, Resource)]
pub struct State {
    instrument: Instrument,
    stations_after: Option<Id>,
    offers_after: Option<(Id, economy::Currency)>,
    owner: Option<Principal>,
    before: Option<u64>,
    orders_after: Option<Id>,
    side: Side,
    quantity: String,
    price: String,
    immediate: bool,
    interval_ms: i64,
    pub orders: bool,
    search: String,
    history: bool,
    bids: bool,
    order_status: Option<osg_model::market::OrderStatus>,
}

impl State {
    pub fn new(account: AccountId) -> Self {
        Self {
            owner: Some(Principal::Player(account)),
            ..Default::default()
        }
    }

    pub fn open_storage(&mut self, owner: Principal, station: Id, item: industry_model::CargoItem) {
        self.owner = Some(owner);
        self.instrument = Instrument::Commodity {
            station,
            item,
            currency: economy::Currency::Uec,
        };
        self.before = None;
        self.orders_after = None;
        self.orders = false;
    }

    pub fn query(&self, open: bool, account: Id) -> Option<MarketQuery> {
        open.then(|| MarketQuery {
            orders_after: self.orders_after,
            order_status: self
                .order_status
                .or(Some(osg_model::market::OrderStatus::Open)),
            stations_after: self.stations_after,
            offers_after: self.offers_after,
            instrument: self.instrument.clone(),
            owner: self.owner.unwrap_or(Principal::Player(account)),
            before: self.before,
            limit: 100,
        })
    }
}

fn quantity_label(instrument: &Instrument, quantity: u64) -> String {
    if *instrument == Instrument::Fx {
        format_amount(quantity)
    } else {
        quantity.to_string()
    }
}

fn item_name(item: &industry_model::CargoItem) -> String {
    let label = cargo::item_label(item).replace('_', " ");
    let mut letters = label.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + letters.as_str()
    })
}

fn instrument_picker(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    snapshot: &MarketView,
) {
    let stations = snapshot.stations.as_slice();
    let previous = state.instrument.clone();
    let items: std::collections::BTreeSet<_> = model
        .industry
        .catalogue
        .as_ref()
        .into_iter()
        .flat_map(|catalogue| &catalogue.recipes)
        .flat_map(|recipe| recipe.inputs.iter().chain(&recipe.outputs))
        .map(|stack| stack.item.clone())
        .collect();

    ui.weak("RESOURCES & PART KITS");
    let mut choices = items;
    if let Instrument::Commodity { item, .. } = &state.instrument {
        choices.insert(item.clone());
    }
    for item in choices {
        let label = item_name(&item);
        if !label.to_lowercase().contains(&state.search.to_lowercase()) {
            continue;
        }
        let active = matches!(&state.instrument, Instrument::Commodity { item: current, .. } if *current == item)
            && !state.orders;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.), egui::Sense::click());
        if active || response.hovered() {
            ui.painter().rect_filled(
                rect,
                0.,
                ACCENT.gamma_multiply(if active { 0.22 } else { 0.10 }),
            );
        }
        if active {
            ui.painter().line_segment(
                [rect.left_top(), rect.left_bottom()],
                egui::Stroke::new(2., ACCENT),
            );
        }
        ui.painter().text(
            rect.left_center() + egui::vec2(10., 0.),
            egui::Align2::LEFT_CENTER,
            osg_ui::icons::Icon::Cargo.glyph(),
            osg_ui::icons::Icon::font(16.),
            ACCENT,
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(32., 0.),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.),
            TEXT,
        );
        if response.clicked() {
            if let Some(station) = stations.first() {
                state.instrument = Instrument::Commodity {
                    station: station.id,
                    item,
                    currency: economy::Currency::Uec,
                };
                state.orders = false;
            }
        }
    }
    ui.add_space(16.);
    ui.weak("CURRENCY");
    ui.vertical(|ui| {
        if ui
            .selectable_label(state.instrument == Instrument::Fx, "UEC / LAT")
            .clicked()
        {
            state.instrument = Instrument::Fx;
            state.orders = false;
        }
    });
    ui.add_space(16.);
    ui.weak("YOU");
    if ui.selectable_label(state.orders, "≡  My orders").clicked() {
        state.orders = true;
    }

    if previous != state.instrument {
        state.before = None;
        state.orders_after = None;
        state.offers_after = None;
    }
}

fn storage(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    market: &MarketView,
    owner: Principal,
    intents: &mut Vec<Intent>,
) {
    let Instrument::Commodity { station, item, .. } = &market.instrument else {
        return;
    };
    ui.heading("Station storage");
    for stock in &market.stock {
        ui.label(format!(
            "{} · {} stored · {} reserved",
            item_name(&stock.item),
            stock.quantity,
            stock.reserved
        ));
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Move quantity");
        ui.add(egui::TextEdit::singleline(&mut state.quantity).desired_width(100.));
        let quantity = state
            .quantity
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|quantity| *quantity > 0);
        for (deposit, label) in [
            (true, "Deposit from focused ship"),
            (false, "Withdraw to focused ship"),
        ] {
            if ui
                .add_enabled(
                    model.connected && model.ship.is_some() && quantity.is_some(),
                    egui::Button::new(label),
                )
                .clicked()
            {
                intents.push(Intent::Market(MarketCommand::MoveStorage {
                    owner,
                    station: *station,
                    ship: model.ship.unwrap().ship,
                    item: item.clone(),
                    quantity: quantity.unwrap(),
                    deposit,
                }));
            }
        }
    });
    ui.weak("Your ship must be docked here. Trades deliver into the buyer’s station storage.");
}

pub fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    snapshot: &MarketView,
    intents: &mut Vec<Intent>,
) {
    ui.painter()
        .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, SURFACE);
    let rail = if ui.available_width() > 1000. {
        220.
    } else {
        155.
    };
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(rail, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.heading("Market");
                ui.add(
                    egui::TextEdit::singleline(&mut state.search)
                        .hint_text("Search items…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.);
                egui::ScrollArea::vertical()
                    .id_salt("market_instrument_picker")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        instrument_picker(ui, state, model, snapshot);
                    });
            },
        );
        ui.separator();
        ui.vertical(|ui| {
            market_body(ui, state, model, snapshot, intents);
        });
    });
}

fn market_body(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    snapshot: &MarketView,
    intents: &mut Vec<Intent>,
) {
    let directory = &model.society.directory;
    let owner = state
        .owner
        .unwrap_or(Principal::Player(model.society.account));
    ui.horizontal_wrapped(|ui| {
        ui.heading(if state.orders {
            "My orders".into()
        } else {
            match &state.instrument {
                Instrument::Fx => "UEC / LAT".into(),
                Instrument::Commodity { item, .. } => item_name(item),
            }
        });
    });
    egui::ComboBox::from_id_salt("market_owner")
        .selected_text(society::name(directory, owner))
        .show_ui(ui, |ui| {
            let owners = std::iter::once(Principal::Player(model.society.account))
                .chain(
                    directory
                        .organizations
                        .keys()
                        .copied()
                        .map(Principal::Organization),
                )
                .chain(
                    directory
                        .sovereignties
                        .keys()
                        .copied()
                        .map(Principal::Sovereignty),
                );
            for owner in owners.filter(|owner| directory.administers(model.society.account, *owner))
            {
                if ui
                    .selectable_value(
                        &mut state.owner,
                        Some(owner),
                        society::name(directory, owner),
                    )
                    .changed()
                {
                    state.before = None;
                    state.orders_after = None;
                }
            }
        });
    if snapshot.owner != Some(owner) || snapshot.instrument != state.instrument {
        return;
    }
    let market = snapshot;
    let price = market
        .last_price
        .map_or_else(|| "No trades yet".into(), format_amount);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(&price)
                .color(MINT)
                .size(22.)
                .monospace(),
        );
        ui.weak(format!(
            "{} · last market trade",
            market.instrument.currency()
        ));
        ui.weak(format!(
            "Available  {} UEC  ·  {} LAT",
            format_amount(market.available_uec),
            format_amount(market.available_lat)
        ));
    });
    egui::ScrollArea::vertical().id_salt("market_content").show(ui, |ui| {
        if market.lat_restricted {
            egui::Frame::new().fill(WARNING.gamma_multiply(0.12)).stroke(egui::Stroke::new(1., WARNING)).inner_margin(8.).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(WARNING, "LAT licence required · LAT offers are shown for reference. Incoming LAT sells into market bids.");
            });
        }
        if state.orders {
            open_orders(ui, state, model, market, intents);
            ui.separator();
            chart(ui, state, market);
        } else if market.instrument == Instrument::Fx {
            let width = ui.available_width();
            let book_width = (width * 0.35).clamp(235., 320.);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(egui::vec2((width - book_width - 12.).max(150.), 0.), egui::Layout::top_down(egui::Align::Min), |ui| {
                    chart(ui, state, market);
                    recent_trades(ui, state, market);
                });
                ui.allocate_ui_with_layout(egui::vec2(book_width, 0.), egui::Layout::top_down(egui::Align::Min), |ui| {
                    if ui.ctx().content_rect().width() < 1100. {
                        ticket(ui, state, model, market, owner, intents);
                        ui.add_space(8.);
                        depth(ui, market);
                    } else {
                        depth(ui, market);
                        ticket(ui, state, model, market, owner, intents);
                    }
                });
            });
        } else {
            if ui.ctx().content_rect().width() < 1100. {
                egui::ScrollArea::vertical().id_salt("commodity_offers").max_height(170.).show(ui, |ui| commodity_offers(ui, state, market));
            } else {
                commodity_offers(ui, state, market);
            }
            if state.history {
                chart(ui, state, market);
            } else {
                ui.separator();
                let width = ui.available_width();
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(width * 0.30, 0.), egui::Layout::top_down(egui::Align::Min), |ui| depth(ui, market));
                    ui.allocate_ui_with_layout(egui::vec2(width * 0.70 - 12., 0.), egui::Layout::top_down(egui::Align::Min), |ui| ticket(ui, state, model, market, owner, intents));
                });
            }
            egui::CollapsingHeader::new("Station storage & cargo transfer").show(ui, |ui| storage(ui, state, model, market, owner, intents));
        }
    });
}

fn chart(ui: &mut egui::Ui, state: &mut State, market: &MarketView) {
    if state.interval_ms == 0 {
        state.interval_ms = 60_000;
    }
    ui.horizontal_wrapped(|ui| {
        for (interval, label) in [
            (60_000, "1m"),
            (300_000, "5m"),
            (900_000, "15m"),
            (3_600_000, "1h"),
            (86_400_000, "1d"),
        ] {
            ui.selectable_value(&mut state.interval_ms, interval, label);
        }
    });
    let samples: Vec<_> = market
        .trades
        .iter()
        .rev()
        .map(|trade| (trade.time_ms, trade.price, trade.quantity))
        .collect();
    components::price_chart(
        ui,
        "market_chart",
        &samples,
        state.interval_ms,
        market.instrument.quantity_scale(),
        if state.orders {
            260.
        } else if ui.ctx().content_rect().width() < 1100. {
            220.
        } else {
            380.
        },
    );
}

fn depth(ui: &mut egui::Ui, market: &MarketView) {
    ui.weak("ORDER BOOK");
    let width = ui.available_width();
    let max = market
        .asks
        .iter()
        .chain(&market.bids)
        .map(|order| order.remaining)
        .max()
        .unwrap_or(1)
        .max(1);
    let widths = [width * 0.45 - 4., width * 0.55 - 4.];
    components::table_row(
        ui,
        &widths,
        &[
            egui::RichText::new("Price").color(MUTED),
            egui::RichText::new("Quantity").color(MUTED),
        ],
        false,
    );
    for (side, orders) in [(Side::Sell, &market.asks), (Side::Buy, &market.bids)] {
        if side == Side::Buy {
            egui::Frame::new()
                .fill(SURFACE_RAISED)
                .inner_margin(4.)
                .show(ui, |ui| {
                    ui.set_width((width - 8.).max(0.));
                    ui.colored_label(
                        MINT,
                        market
                            .last_price
                            .map_or_else(|| "Spread".into(), format_amount),
                    );
                });
        }
        let mut levels = std::collections::BTreeMap::<u64, u64>::new();
        for order in orders {
            let quantity = levels.entry(order.price).or_default();
            *quantity = quantity.saturating_add(order.remaining);
        }
        let count = if market.instrument == Instrument::Fx {
            6
        } else {
            3
        };
        let levels: Vec<_> = if side == Side::Sell {
            levels
                .into_iter()
                .take(count)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        } else {
            levels.into_iter().rev().take(count).collect()
        };
        for (price, quantity) in levels {
            let color = if side == Side::Buy { POSITIVE } else { THREAT };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 18.), egui::Sense::hover());
            let bar = egui::Rect::from_min_max(
                egui::pos2(
                    rect.right() - width * (quantity as f32 / max as f32).min(1.),
                    rect.top(),
                ),
                rect.right_bottom(),
            );
            ui.painter()
                .rect_filled(bar, 0., color.gamma_multiply(0.14));
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                format_amount(price),
                egui::FontId::monospace(12.),
                color,
            );
            ui.painter().text(
                rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                quantity_label(&market.instrument, quantity),
                egui::FontId::monospace(12.),
                TEXT,
            );
        }
    }
    if market.instrument == Instrument::Fx {
        ui.weak(format!(
            "USE bid {} · unlimited UEC",
            format_amount(market.backstop_price)
        ));
    }
    ui.add_space(10.);
}

fn ticket(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    market: &MarketView,
    owner: Principal,
    intents: &mut Vec<Intent>,
) {
    egui::Frame::new().fill(SURFACE_RAISED).inner_margin(10.).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.side, Side::Buy, "Buy");
            ui.selectable_value(&mut state.side, Side::Sell, "Sell");
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut state.immediate, false, "Limit");
            ui.selectable_value(&mut state.immediate, true, "Immediate");
        });
        ui.columns(2, |columns| {
            columns[0].weak(format!("Limit · {}", market.instrument.currency()));
            columns[0].add(egui::TextEdit::singleline(&mut state.price).desired_width(f32::INFINITY).hint_text("Price"));
            columns[1].weak(if market.instrument == Instrument::Fx { "Amount · LAT" } else { "Quantity · units" });
            columns[1].add(egui::TextEdit::singleline(&mut state.quantity).desired_width(f32::INFINITY).hint_text("Quantity"));
        });
        let quantity = if market.instrument == Instrument::Fx {
            parse_amount(&state.quantity)
        } else {
            state.quantity.trim().parse().ok()
        };
        let values = quantity
            .zip(parse_amount(&state.price))
            .filter(|(quantity, price)| *quantity > 0 && *price > 0);

        if let Some((quantity, price)) = values {
            let value = u128::from(quantity) * u128::from(price) / u128::from(market.instrument.quantity_scale());
            if let Ok(value) = u64::try_from(value) {
                ui.horizontal(|ui| {
                    ui.weak("Order value");
                    ui.colored_label(ACCENT, format!("{} {}", format_amount(value), market.instrument.currency()));
                });
            }
        }
        ui.weak(format!("Pay from {}", society::name(&model.society.directory, owner)));
        if market.instrument != Instrument::Fx {
            ui.weak("Delivery into your storage at the selected station. Arrange transport separately.");
        }
        let licensed = !market.lat_restricted
            || !(market.instrument == Instrument::Fx
                || state.side == Side::Buy && market.instrument.currency() == economy::Currency::Lat);
        let color = if state.side == Side::Buy { POSITIVE } else { THREAT };
        let button = egui::Button::new(
            egui::RichText::new(format!("Place {:?} order", state.side)).color(color),
        )
        .fill(color.gamma_multiply(0.18))
        .min_size(egui::vec2(ui.available_width(), 28.));

        if ui.add_enabled(model.connected && licensed && values.is_some(), button).clicked() {
            let (quantity, price) = values.unwrap();
            let command = if state.immediate {
                MarketCommand::Immediate { instrument: market.instrument.clone(), owner, side: state.side, quantity, price }
            } else {
                MarketCommand::Limit { instrument: market.instrument.clone(), owner, side: state.side, quantity, price }
            };
            intents.push(Intent::Market(command));
        }
        ui.weak("Turnover tax applies to transfers. Open orders reserve funds or goods.");
    });
}

fn commodity_offers(ui: &mut egui::Ui, state: &mut State, market: &MarketView) {
    ui.horizontal(|ui| {
        if ui
            .selectable_label(!state.history && !state.bids, "Offers")
            .clicked()
        {
            state.history = false;
            state.bids = false;
        }
        if ui
            .selectable_label(!state.history && state.bids, "Bids")
            .clicked()
        {
            state.history = false;
            state.bids = true;
        }
        ui.selectable_value(&mut state.history, true, "History");
    });
    if state.history {
        return;
    }
    let width = ui.available_width();
    let widths = [0.40, 0.12, 0.16, 0.16, 0.16].map(|f| (width - 32.) * f);
    let headers = [
        "Station",
        "Currency",
        "From / unit",
        "≈ UEC / unit",
        "Available",
    ];
    components::table_row(
        ui,
        &widths,
        &headers.map(|s| egui::RichText::new(s).color(MUTED)),
        false,
    );
    let Instrument::Commodity {
        station,
        item,
        currency,
    } = &state.instrument
    else {
        return;
    };
    let selected_station = *station;
    let selected_currency = *currency;
    let item = item.clone();
    for (index, offer) in market.offers.iter().enumerate() {
        let selected = offer.station.id == selected_station && offer.currency == selected_currency;
        let restricted = market.lat_restricted && offer.currency == economy::Currency::Lat;
        let height = 30.;
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, height));
        if selected {
            ui.painter()
                .rect_filled(rect, 0., ACCENT.gamma_multiply(0.22));
        }
        let color = if restricted { MUTED } else { TEXT };
        let texts = [
            format!("◇  {}", offer.station.name),
            offer.currency.to_string(),
            (if state.bids {
                offer.bid_price
            } else {
                offer.price
            })
            .map_or_else(|| "—".into(), format_amount),
            (if state.bids {
                offer.comparable_bid_uec
            } else {
                offer.comparable_uec
            })
            .map_or_else(|| "—".into(), format_amount),
            (if state.bids {
                offer.bid_quantity
            } else {
                offer.available
            })
            .to_string(),
        ];
        let cells: Vec<_> = texts
            .iter()
            .map(|s| egui::RichText::new(s).color(color))
            .collect();
        components::table_row(ui, &widths, &cells, !selected && index % 2 == 0);
        if ui
            .interact(
                rect,
                ui.id()
                    .with(("offer", offer.station.id, offer.currency as u8)),
                egui::Sense::click(),
            )
            .clicked()
        {
            state.instrument = Instrument::Commodity {
                station: offer.station.id,
                item: item.clone(),
                currency: offer.currency,
            };
            state.before = None;
            state.price = offer.price.map(format_amount).unwrap_or_default();
        }
    }
    if market.offers.is_empty() {
        ui.weak("No open station offers for this item.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.offers_after.is_some(),
                egui::Button::new("First offers"),
            )
            .clicked()
        {
            state.offers_after = None;
        }
        if ui
            .add_enabled(
                market.offers_next.is_some(),
                egui::Button::new("More offers"),
            )
            .clicked()
        {
            state.offers_after = market.offers_next;
        }
    });
}

fn open_orders(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    market: &MarketView,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        let previous = state.order_status;
        for (status, label) in [
            (osg_model::market::OrderStatus::Open, "Open"),
            (osg_model::market::OrderStatus::Completed, "Filled"),
            (osg_model::market::OrderStatus::Cancelled, "Cancelled"),
        ] {
            if ui
                .selectable_label(
                    state
                        .order_status
                        .unwrap_or(osg_model::market::OrderStatus::Open)
                        == status,
                    label,
                )
                .clicked()
            {
                state.order_status = Some(status);
            }
        }
        if previous != state.order_status {
            state.orders_after = None;
        }
    });
    ui.weak("Buy orders hold currency; sell orders hold goods or LAT");
    egui::Grid::new("my_orders")
        .striped(true)
        .min_col_width(55.)
        .show(ui, |ui| {
            for title in ["Side", "Item / station", "Price", "Filled", ""] {
                ui.weak(title);
            }
            ui.end_row();
            for order in &market.orders {
                let color = if order.side == Side::Buy {
                    POSITIVE
                } else {
                    THREAT
                };
                components::badge(
                    ui,
                    if order.side == Side::Buy {
                        "BUY"
                    } else {
                        "SELL"
                    },
                    color,
                );
                ui.label(match &order.instrument {
                    Instrument::Fx => "UEC / LAT".into(),
                    Instrument::Commodity { station, item, .. } => format!(
                        "{} · {}",
                        item_name(item),
                        market
                            .stations
                            .iter()
                            .find(|s| s.id == *station)
                            .map_or("Station", |s| s.name.as_str())
                    ),
                });
                ui.monospace(format!(
                    "{} {}",
                    format_amount(order.price),
                    order.instrument.currency()
                ));
                ui.add(
                    egui::ProgressBar::new(
                        order.filled_quantity as f32 / order.original_quantity.max(1) as f32,
                    )
                    .desired_width(100.)
                    .fill(ACCENT)
                    .text(format!(
                        "{} / {}",
                        quantity_label(&order.instrument, order.filled_quantity),
                        quantity_label(&order.instrument, order.original_quantity)
                    )),
                );
                if ui
                    .add_enabled(
                        model.connected && order.status == osg_model::market::OrderStatus::Open,
                        egui::Button::new("Cancel"),
                    )
                    .clicked()
                {
                    intents.push(Intent::Market(MarketCommand::Cancel { order: order.id }));
                }
                ui.end_row();
            }
        });
    if market.orders.is_empty() {
        ui.weak("No open orders.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.orders_after.is_some(),
                egui::Button::new("First page"),
            )
            .clicked()
        {
            state.orders_after = None;
        }
        if ui
            .add_enabled(market.orders_next.is_some(), egui::Button::new("Next page"))
            .clicked()
        {
            state.orders_after = market.orders_next;
        }
    });
}

fn recent_trades(ui: &mut egui::Ui, state: &mut State, market: &MarketView) {
    ui.colored_label(ACCENT, "Recent trades");
    let widths = [0.50, 0.25, 0.25].map(|f| (ui.available_width() - 16.) * f);
    components::table_row(
        ui,
        &widths,
        &[
            egui::RichText::new("Time · UTC").color(MUTED),
            egui::RichText::new("Price").color(MUTED),
            egui::RichText::new("Volume").color(MUTED),
        ],
        false,
    );
    for (index, trade) in market.trades.iter().take(5).enumerate() {
        let date = osg_model::calendar::format_utc(trade.time_ms);
        let texts = [
            date[4..23].to_owned(),
            format_amount(trade.price),
            quantity_label(&market.instrument, trade.quantity),
        ];
        components::table_row(
            ui,
            &widths,
            &texts.map(|text| egui::RichText::new(text).monospace().color(MUTED)),
            index % 2 == 0,
        );
    }
    ui.horizontal(|ui| {
        if ui.small_button("Latest").clicked() {
            state.before = None;
        }
        if ui
            .add_enabled(
                market.next_before.is_some(),
                egui::Button::new("Older trades"),
            )
            .clicked()
        {
            state.before = market.next_before;
        }
    });
}
