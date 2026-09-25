use super::*;
use osg_model::{industry::*, market::StoredStock, ownership::Principal};

#[derive(Clone, PartialEq)]
pub(crate) struct Query {
    pub search: String,
    pub after: Option<Id>,
    pub facility: Option<Id>,
    pub payer: Principal,
    pub work: Option<ServiceWork>,
}

#[derive(Clone, Default)]
pub(crate) struct View {
    pub facilities: Vec<PublicFacility>,
    pub next: Option<Id>,
    pub jobs: Vec<JobView>,
    pub stock: Vec<StoredStock>,
    pub quote: Option<ServiceQuote>,
    pub quotes: std::collections::BTreeMap<Id, Result<ServiceQuote, String>>,
    pub available: u64,
    pub error: Option<String>,
}

pub(super) async fn fetch(client: OsgNetClient, world: Id, query: Query) -> Result<View, String> {
    let page = call(client.list_public_facilities(world, query.search, query.after, 128)).await?;
    let mut view = View {
        facilities: page.items,
        next: page.next,
        ..Default::default()
    };
    if let Some(work) = &query.work {
        // Bound simultaneous requests while retaining each facility's refusal reason.
        for facilities in view.facilities.chunks(8) {
            let mut pending = tokio::task::JoinSet::new();
            for facility in facilities {
                let client = client.clone();
                let work = work.clone();
                let id = facility.summary.entity;
                let payer = query.payer;
                pending.spawn(async move {
                    (
                        id,
                        call(client.quote_industry_job(world, id, payer, work)).await,
                    )
                });
            }
            while let Some(result) = pending.join_next().await {
                let (id, quote) = result.map_err(|error| error.to_string())?;
                view.quotes.insert(id, quote);
            }
        }
    }
    if let Some(facility) = query.facility {
        let (jobs, stock, wallet) = tokio::try_join!(
            call(client.service_jobs(world, facility)),
            call(client.storage_stock(world, query.payer, facility)),
            call(client.wallet_balance(world, query.payer)),
        )?;
        view.jobs = jobs;
        view.stock = stock;
        if query.work.is_some() {
            match view.quotes.get(&facility).cloned() {
                Some(Ok(quote)) => {
                    view.available = match quote.currency {
                        economy::Currency::Uec => wallet
                            .balance
                            .uec
                            .saturating_sub(wallet.balance.reserved_uec),
                        economy::Currency::Lat => wallet
                            .balance
                            .lat
                            .saturating_sub(wallet.balance.reserved_lat),
                    };
                    view.quote = Some(quote);
                }
                Some(Err(error)) => view.error = Some(error),
                None => {}
            }
        }
    }
    Ok(view)
}
