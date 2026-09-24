use super::*;
use economy::{Currency, WalletCommand, WalletQuery, WalletSnapshot, format_amount, parse_amount};
use osg_ui::components;
use ownership::Principal;

#[derive(Default)]
pub(super) struct State {
    owner: Option<Principal>,
    before: Option<u64>,
    currency: Option<Currency>,
    search: String,
    kind: Option<economy::EntryKind>,
    transfer: bool,
    internal: bool,
    recipient: Option<Principal>,
    amount: String,
    send_currency: Currency,
    gas: bool,
    tax_owner: Option<Principal>,
    tax_basis_points: u16,
}

impl State {
    pub fn query(&self, open: bool, account: Id) -> Option<WalletQuery> {
        open.then(|| WalletQuery {
            owner: self.owner.unwrap_or(Principal::Player(account)),
            before: self.before,
            limit: 100,
        })
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    snapshot: Option<&WalletSnapshot>,
    intents: &mut Vec<Intent>,
) {
    ui.painter()
        .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, SURFACE);
    components::action_header(
        ui,
        "wallet_header",
        "Wallet",
        "You and the organizations you administer",
        |ui| {
            if ui
                .add_enabled(model.connected, egui::Button::new("Move between accounts…"))
                .clicked()
            {
                state.transfer = true;
                state.internal = true;
                state.recipient = None;
            }
            if ui
                .add_enabled(
                    model.connected,
                    egui::Button::new("Send…").fill(ui.visuals().selection.bg_fill),
                )
                .clicked()
            {
                state.transfer = !state.transfer;
                state.internal = false;
                state.recipient = None;
            }
        },
    );
    let Some(wallet) = snapshot else {
        components::empty_state(
            ui,
            "Loading wallet",
            "Waiting for account balances and ledger history.",
        );
        return;
    };
    if let Some(error) = &wallet.error {
        ui.colored_label(THREAT, error);
        return;
    }
    let owner = state
        .owner
        .unwrap_or(Principal::Player(model.society.account));
    let balance = wallet
        .balances
        .iter()
        .find(|balance| balance.owner == owner);
    if state.tax_owner != Some(owner) {
        state.tax_owner = Some(owner);
        state.tax_basis_points = balance.map_or(0, |balance| balance.turnover_tax_bps);
    }
    let next = balance.map_or(0, |b| b.next_demurrage);
    let market_value = wallet.market_uec_per_lat.map_or_else(
        || "Market price unavailable".into(),
        |price| format!("Market: 1 LAT = {} UEC", format_amount(price)),
    );
    let width = ui.available_width();
    let gap = ui.spacing().item_spacing.x;
    let widths = [
        (width - 2. * gap) * 0.38,
        (width - 2. * gap) * 0.38,
        (width - 2. * gap) * 0.24,
    ];
    ui.horizontal_top(|ui| {
        for (index, title, accent) in [(0, "UEC  Union Economic Credits", INFO), (1, "LAT  Latinum", MINT), (2, "Gas", ACCENT)] {
            let restricted = index == 1 && balance.is_some_and(|b| b.lat_restricted);
            egui::Frame::new().fill(SURFACE_RAISED).stroke(egui::Stroke::new(1., if restricted { WARNING } else { BORDER })).inner_margin(10.).show(ui, |ui| {
                ui.vertical(|ui| {
                ui.set_width((widths[index] - 22.).max(70.));
                ui.set_min_height(142.);
                ui.colored_label(accent, title);
                if index == 2 {
                    ui.weak("Computer execution");
                    for gas in &model.society.gas_accounts {
                        account_row(ui, &society::name(&model.society.directory, gas.owner), &grouped(&gas.available.to_string()), ACCENT);
                    }
                    ui.separator();
                    if let Some(gas) = model.society.gas_accounts.iter().find(|b| b.owner == owner) {
                        let total = gas.available.saturating_add(gas.reserved).max(1);
                        ui.weak("Reserved / current allocation");
                        ui.add(egui::ProgressBar::new(gas.reserved as f32 / total as f32).fill(ACCENT).desired_height(4.));
                        account_row(ui, "Reserved", &grouped(&gas.reserved.to_string()), MUTED);
                    }
                    ui.weak("Movable between accounts; not exchangeable.");
                } else {
                    if restricted {
                        ui.colored_label(WARNING, "LAT licence required");
                        ui.weak("Incoming LAT is sold for UEC on the market. You cannot hold LAT without a licence.");
                    } else {
                        for account in &wallet.balances {
                            let amount = if index == 0 { account.uec } else { account.lat };
                            account_row(ui, &society::name(&model.society.directory, account.owner), &grouped(&format_amount(amount)), TEXT);
                        }
                    }
                    ui.separator();
                    if index == 0 {
                        ui.weak("Official currency of the Union State of Earth");
                        account_row(ui, "Demurrage", "20% / year · daily", WARNING);
                        ui.weak("Above 50,000 UEC per account");
                        account_row(ui, "Next charge", &format!("{} UEC", grouped(&format_amount(next))), WARNING);
                    } else {
                        ui.weak("De-facto standard cross-border currency");
                        ui.label(&market_value);
                        sparkline(ui, &wallet.fx_trades);
                    }
                }
                });
            });
        }
    });
    ui.add_space(8.);
    let tax_rate = balance.map_or(0, |balance| balance.turnover_tax_bps);
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .inner_margin(8.)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ACCENT, "TURNOVER TAX");
                ui.colored_label(WARNING, format!(
            "Turnover tax: {}.{:02}% on receipts, including transfers and FX conversions",
            tax_rate / 100,
            tax_rate % 100,
            ));
            });
        });
    if let Principal::Sovereignty(sovereignty) = owner {
        if model
            .society
            .directory
            .administers(model.society.account, owner)
        {
            ui.horizontal_wrapped(|ui| {
                ui.label("Set turnover tax (100 basis points = 1%)");
                ui.add(egui::DragValue::new(&mut state.tax_basis_points).range(0..=10_000));
                if ui
                    .add_enabled(
                        model.connected && state.tax_basis_points != tax_rate,
                        egui::Button::new("Set tax rate"),
                    )
                    .clicked()
                {
                    intents.push(Intent::Wallet(WalletCommand::SetTurnoverTax {
                        sovereignty,
                        basis_points: state.tax_basis_points,
                    }));
                }
            });
        }
    }
    let previous = state.owner;
    ui.add_space(8.);
    ui.weak("LEDGER");
    components::filter_bar(ui, "wallet_filter", &mut state.search, |ui| {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("wallet_account")
                .selected_text(society::name(&model.society.directory, owner))
                .show_ui(ui, |ui| {
                    for balance in &wallet.balances {
                        ui.selectable_value(
                            &mut state.owner,
                            Some(balance.owner),
                            society::name(&model.society.directory, balance.owner),
                        );
                    }
                });
            for (currency, label) in [
                (None, "All"),
                (Some(Currency::Uec), "UEC"),
                (Some(Currency::Lat), "LAT"),
            ] {
                ui.selectable_value(&mut state.currency, currency, label);
            }
            egui::ComboBox::from_id_salt("wallet_entry_kind")
                .selected_text(
                    state
                        .kind
                        .map_or_else(|| "All kinds".into(), |kind| format!("{kind:?}")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.kind, None, "All kinds");
                    for kind in [
                        economy::EntryKind::Transfer,
                        economy::EntryKind::Market,
                        economy::EntryKind::Industry,
                        economy::EntryKind::Conversion,
                        economy::EntryKind::Tax,
                        economy::EntryKind::Fee,
                        economy::EntryKind::Demurrage,
                        economy::EntryKind::Reserve,
                        economy::EntryKind::Release,
                        economy::EntryKind::Issue,
                    ] {
                        ui.selectable_value(&mut state.kind, Some(kind), format!("{kind:?}"));
                    }
                });
        });
    });
    if previous != state.owner {
        state.before = None;
    }
    if state.transfer {
        transfer_form(ui, state, owner, model, wallet, intents);
    }
    ui.separator();
    if wallet.owner != Some(state.owner.unwrap_or(owner)) {
        ui.weak("Loading selected account…");
        return;
    }
    let search = state.search.to_lowercase();
    let entries: Vec<_> = wallet
        .entries
        .iter()
        .filter(|entry| {
            state
                .currency
                .is_none_or(|currency| entry.currency == currency)
                && state.kind.is_none_or(|kind| entry.kind == kind)
                && (search.is_empty()
                    || format!(
                        "{:?} {}",
                        entry.kind,
                        entry
                            .counterparty
                            .map_or(String::new(), |owner| society::name(
                                &model.society.directory,
                                owner
                            ))
                    )
                    .to_lowercase()
                    .contains(&search))
        })
        .collect();
    let width = ui.available_width();
    let widths = [0.15, 0.17, 0.38, 0.15, 0.15].map(|fraction| (width - 32.) * fraction);
    components::table_row_aligned(
        ui,
        &widths,
        &[
            egui::RichText::new("Time (UTC)").color(MUTED),
            egui::RichText::new("Account").color(MUTED),
            egui::RichText::new("Entry").color(MUTED),
            egui::RichText::new("Amount").color(MUTED),
            egui::RichText::new("Balance").color(MUTED),
        ],
        &[false, false, false, true, true],
        false,
    );
    egui::ScrollArea::vertical()
        .id_salt("wallet_ledger")
        .max_height((ui.available_height() - 48.).max(0.))
        .auto_shrink([false, false])
        .show_rows(ui, 24., entries.len(), |ui, range| {
            for index in range {
                let entry = entries[index];
                let date = osg_model::calendar::format_utc(entry.time_ms);
                let cells = [
                    egui::RichText::new(&date[4..20]).color(MUTED),
                    egui::RichText::new(society::name(&model.society.directory, owner)).color(TEXT),
                    egui::RichText::new(format!(
                        "{:?} {}",
                        entry.kind,
                        entry.counterparty.map_or(String::new(), |p| society::name(
                            &model.society.directory,
                            p
                        ))
                    ))
                    .color(
                        if matches!(
                            entry.kind,
                            economy::EntryKind::Demurrage
                                | economy::EntryKind::Tax
                                | economy::EntryKind::Fee
                        ) {
                            WARNING
                        } else {
                            TEXT
                        },
                    ),
                    egui::RichText::new(format!(
                        "{}{} {}",
                        if entry.credit { "+" } else { "−" },
                        grouped(&format_amount(entry.amount)),
                        entry.currency
                    ))
                    .monospace()
                    .color(if entry.credit { POSITIVE } else { WARNING }),
                    egui::RichText::new(format!(
                        "{} {}",
                        grouped(&format_amount(entry.balance)),
                        entry.currency
                    ))
                    .monospace()
                    .color(MUTED),
                ];
                if index % 2 == 0 {
                    let rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(ui.available_width(), 24.),
                    );
                    ui.painter().rect_filled(rect, 0., SURFACE_RAISED);
                }
                ui.horizontal(|ui| {
                    for (column, width) in widths.iter().enumerate() {
                        let layout = if column >= 3 {
                            egui::Layout::right_to_left(egui::Align::Center)
                        } else {
                            egui::Layout::left_to_right(egui::Align::Center)
                        };
                        ui.allocate_ui_with_layout(egui::vec2(*width, 24.), layout, |ui| {
                            ui.set_width(*width);
                            if column == 2 {
                                let color = if matches!(
                                    entry.kind,
                                    economy::EntryKind::Tax
                                        | economy::EntryKind::Fee
                                        | economy::EntryKind::Demurrage
                                ) {
                                    WARNING
                                } else {
                                    MUTED
                                };
                                components::badge(
                                    ui,
                                    &format!("{:?}", entry.kind).to_uppercase(),
                                    color,
                                );
                                let description = match entry.kind {
                                    economy::EntryKind::Issue => "Currency issued",
                                    economy::EntryKind::Transfer => {
                                        if entry.credit {
                                            "Received"
                                        } else {
                                            "Sent"
                                        }
                                    }
                                    economy::EntryKind::Demurrage => "Daily charge above exemption",
                                    economy::EntryKind::Conversion => "LAT / UEC conversion",
                                    economy::EntryKind::Reserve => "Order reservation",
                                    economy::EntryKind::Release => "Reservation released",
                                    economy::EntryKind::Market => "Market settlement",
                                    economy::EntryKind::Industry => "Industry service payment",
                                    economy::EntryKind::Tax => "Turnover tax",
                                    economy::EntryKind::Fee => "Fee",
                                };
                                let text = entry.counterparty.map_or_else(
                                    || description.to_owned(),
                                    |counterparty| {
                                        format!(
                                            "{description} · {}",
                                            society::name(&model.society.directory, counterparty)
                                        )
                                    },
                                );
                                ui.add(egui::Label::new(text).truncate());
                            } else {
                                ui.add(egui::Label::new(cells[column].clone()).truncate());
                            }
                        });
                    }
                });
            }
        });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(state.before.is_some(), egui::Button::new("Latest"))
            .clicked()
        {
            state.before = None;
        }
        if ui
            .add_enabled(
                wallet.next_before.is_some(),
                egui::Button::new("Older entries"),
            )
            .clicked()
        {
            state.before = wallet.next_before;
        }
        ui.weak(format!("{} entries", entries.len()));
    });
}

fn grouped(value: &str) -> String {
    let (whole, fractional) = value
        .split_once('.')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let mut result = String::new();
    for (index, character) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            result.push(',');
        }
        result.push(character);
    }
    if let Some(fractional) = fractional {
        result.push('.');
        result.push_str(fractional);
    }
    result
}

fn sparkline(ui: &mut egui::Ui, trades: &[osg_model::market::Trade]) {
    ui.weak("1 LAT in UEC · recent trades");
    if trades.len() < 2 {
        return;
    }
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.), egui::Sense::hover());
    let low = trades.iter().map(|t| t.price).min().unwrap();
    let high = trades.iter().map(|t| t.price).max().unwrap();
    let range = high.saturating_sub(low).max(1);
    let points: Vec<_> = trades
        .iter()
        .rev()
        .enumerate()
        .map(|(index, trade)| {
            egui::pos2(
                rect.left() + rect.width() * index as f32 / (trades.len() - 1) as f32,
                rect.bottom()
                    - 2.
                    - (rect.height() - 4.) * trade.price.saturating_sub(low) as f32 / range as f32,
            )
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.2, MINT)));
}

fn account_row(ui: &mut egui::Ui, label: &str, value: &str, color: egui::Color32) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 17.), egui::Sense::hover());
    let label_rect = egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.bottom()));
    ui.painter().with_clip_rect(label_rect).text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.),
        MUTED,
    );
    ui.painter().with_clip_rect(rect).text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        value,
        egui::FontId::monospace(11.),
        color,
    );
    response.on_hover_text(format!("{label}: {value}"));
}

fn transfer_form(
    ui: &mut egui::Ui,
    state: &mut State,
    from: Principal,
    model: &FrameModel,
    wallet: &WalletSnapshot,
    intents: &mut Vec<Intent>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label("Transfer from the selected account");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut state.gas, "Gas");
            if !state.gas {
                ui.selectable_value(&mut state.send_currency, Currency::Uec, "UEC");
                ui.selectable_value(&mut state.send_currency, Currency::Lat, "LAT");
            }
            ui.add(
                egui::TextEdit::singleline(&mut state.amount)
                    .hint_text("Amount")
                    .desired_width(150.),
            );
            egui::ComboBox::from_id_salt("wallet_recipient")
                .selected_text(state.recipient.map_or_else(
                    || "Recipient…".into(),
                    |p| society::name(&model.society.directory, p),
                ))
                .show_ui(ui, |ui| {
                    let directory = &model.society.directory;
                    let recipients = directory
                        .players
                        .keys()
                        .copied()
                        .map(Principal::Player)
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
                    for recipient in recipients.filter(|p| {
                        *p != from
                            && (!(state.gas || state.internal)
                                || directory.administers(model.society.account, *p))
                    }) {
                        ui.selectable_value(
                            &mut state.recipient,
                            Some(recipient),
                            society::name(directory, recipient),
                        );
                    }
                });
            let amount = if state.gas {
                state.amount.trim().parse::<u64>().ok()
            } else {
                parse_amount(&state.amount)
            }
            .filter(|amount| *amount > 0);
            let restricted = !state.gas
                && state.send_currency == Currency::Lat
                && wallet
                    .balances
                    .iter()
                    .any(|b| b.owner == from && b.lat_restricted);
            if ui
                .add_enabled(
                    model.connected
                        && !restricted
                        && amount.is_some()
                        && state.recipient.is_some_and(|to| to != from),
                    egui::Button::new("Send"),
                )
                .clicked()
            {
                let to = state.recipient.unwrap();
                let amount = amount.unwrap();
                intents.push(Intent::Wallet(if state.gas {
                    WalletCommand::TransferGas { from, to, amount }
                } else {
                    WalletCommand::Transfer {
                        from,
                        to,
                        currency: state.send_currency,
                        amount,
                    }
                }));
                state.transfer = false;
            }
        });
    });
}
