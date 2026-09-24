//! Currency amounts use millionths of one unit throughout the protocol.
use crate::{Id, ownership::Principal};
use serde::{Deserialize, Serialize};

pub const MONEY_SCALE: u64 = 1_000_000;
pub const DEMURRAGE_EXEMPTION: u64 = 50_000 * MONEY_SCALE;
pub const DAY_MS: i64 = 86_400_000;
pub const RATE_SCALE: u128 = 1_000_000_000_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Currency {
    #[default]
    Uec,
    Lat,
}

impl std::fmt::Display for Currency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Uec => "UEC",
            Self::Lat => "LAT",
        })
    }
}

/// Parse user input without passing monetary values through floating point.
pub fn parse_amount(text: &str) -> Option<u64> {
    let (whole, fraction) = text.trim().split_once('.').unwrap_or((text.trim(), ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let fraction_digits = fraction.len() as u32;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().ok()?
    };
    whole
        .parse::<u64>()
        .ok()?
        .checked_mul(MONEY_SCALE)?
        .checked_add(fraction.checked_mul(10_u64.pow(6 - fraction_digits))?)
}

pub fn format_amount(amount: u64) -> String {
    let whole = amount / MONEY_SCALE;
    let fraction = amount % MONEY_SCALE;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:06}")
            .trim_end_matches('0')
            .to_string()
    }
}

/// Rate for the UTC day being charged, rounded down at 18 decimal places.
/// Compounding over the calendar year retains 80% of the taxable balance,
/// within the fixed point rounding precision.
pub fn daily_rate(day: i64) -> u128 {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month = (5 * doy + 2) / 153;
    let year = yoe + era * 400 + i64::from(month >= 10);
    if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
        609_496_015_987_604
    } else {
        611_165_357_704_478
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletQuery {
    pub owner: Principal,
    pub before: Option<u64>,
    pub limit: u16,
}

impl WalletQuery {
    pub fn valid(&self) -> bool {
        (1..=100).contains(&self.limit)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletCommand {
    SetTurnoverTax {
        sovereignty: Id,
        basis_points: u16,
    },
    Transfer {
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
    },
    TransferGas {
        from: Principal,
        to: Principal,
        amount: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Issue,
    Transfer,
    Demurrage,
    Conversion,
    Reserve,
    Release,
    Market,
    Industry,
    Tax,
    Fee,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub sequence: u64,
    pub time_ms: i64,
    pub owner: Principal,
    pub currency: Currency,
    pub kind: EntryKind,
    pub credit: bool,
    pub amount: u64,
    pub balance: u64,
    pub counterparty: Option<Principal>,
    pub reference: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBalance {
    pub turnover_tax_bps: u16,
    pub owner: Principal,
    pub uec: u64,
    pub lat: u64,
    pub reserved_uec: u64,
    pub reserved_lat: u64,
    pub next_demurrage: u64,
    pub lat_restricted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletSnapshot {
    /// Latest executed FX trades, newest first, for market valuation history.
    pub fx_trades: Vec<crate::market::Trade>,
    pub owner: Option<Principal>,
    pub balances: Vec<WalletBalance>,
    pub entries: Vec<LedgerEntry>,
    pub next_before: Option<u64>,
    pub next_charge_ms: i64,
    /// Price of the latest executed trade, in micro UEC per LAT.
    pub market_uec_per_lat: Option<u64>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monetary_input_roundtrips_exactly_and_rejects_precision_loss() {
        for amount in [0, 1, 999_999, MONEY_SCALE, u64::MAX] {
            assert_eq!(parse_amount(&format_amount(amount)), Some(amount));
        }
        for invalid in [
            "",
            "NaN",
            "inf",
            "-1",
            "1e9",
            "1.0000001",
            "18446744073709.551616",
            "1.2.3",
        ] {
            assert_eq!(parse_amount(invalid), None, "{invalid}");
        }
    }
}
