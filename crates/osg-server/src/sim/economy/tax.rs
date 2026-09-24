use super::*;
use osg_model::ownership::OwnershipDirectory;

#[cfg(test)]
mod tests;

impl Economy {
    pub fn tax_rate(
        &self,
        directory: &OwnershipDirectory,
        recipient: Principal,
    ) -> Option<(Principal, u16)> {
        directory
            .lineage(recipient)
            .into_iter()
            .find_map(|principal| {
                let Principal::Sovereignty(id) = principal else {
                    return None;
                };
                self.turnover_taxes.get(&id).map(|rate| (principal, *rate))
            })
    }
}

impl Transaction<'_> {
    /// Withhold turnover tax from the receipt. Tax remittance is bookkeeping,
    /// so it does not recursively levy another tax on the same charge.
    pub(super) fn levy_turnover(
        &mut self,
        directory: Option<&OwnershipDirectory>,
        source: Option<Principal>,
        recipient: Principal,
        currency: Currency,
        gross: u64,
        now: i64,
    ) -> Result<u64> {
        if self.turnover_taxes.is_empty() && (source.is_none() || directory.is_none()) {
            return Ok(gross);
        }

        let directory = directory.context("tax jurisdiction unavailable")?;
        let mut remaining = gross;
        if let Some((collector, rate)) = self.tax_rate(directory, recipient) {
            if recipient != collector && rate > 0 {
                remaining -= self.levy(recipient, collector, currency, gross, rate, now)?;
            }
        }
        if let Some(source) = source {
            let tariff = directory
                .lineage(source)
                .into_iter()
                .flat_map(|collector| {
                    directory
                        .lineage(recipient)
                        .into_iter()
                        .flat_map(move |partner| {
                            directory
                                .diplomacy
                                .active_terms(collector, partner)
                                .filter_map(move |term| {
                                    if let osg_model::diplomacy::AgreementTerm::Tariff {
                                        basis_points,
                                    } = term
                                    {
                                        Some((collector, *basis_points))
                                    } else {
                                        None
                                    }
                                })
                        })
                })
                .max_by_key(|(_, rate)| *rate);
            if let Some((collector, rate)) = tariff {
                if recipient != collector && rate > 0 {
                    remaining -= self.levy(recipient, collector, currency, remaining, rate, now)?;
                }
            }
        }
        Ok(remaining)
    }

    fn levy(
        &mut self,
        recipient: Principal,
        collector: Principal,
        currency: Currency,
        gross: u64,
        rate: u16,
        now: i64,
    ) -> Result<u64> {
        let tax = (gross as u128 * rate as u128).div_ceil(10_000) as u64;
        let treasury = self.balance_mut(collector);
        let treasury_amount = match currency {
            Currency::Uec => &mut treasury.uec,
            Currency::Lat => &mut treasury.lat,
        };
        *treasury_amount = treasury_amount
            .checked_add(tax)
            .context("treasury overflow")?;

        let payer = self.balance_mut(recipient);
        let balance = match currency {
            Currency::Uec => &mut payer.uec,
            Currency::Lat => &mut payer.lat,
        };
        *balance = balance.checked_sub(tax).context("tax exceeds receipt")?;

        self.record(
            recipient,
            currency,
            false,
            tax,
            EntryKind::Tax,
            Some(collector),
            now,
        );
        self.record(
            collector,
            currency,
            true,
            tax,
            EntryKind::Tax,
            Some(recipient),
            now,
        );
        Ok(tax)
    }
}
