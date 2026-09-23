use std::fmt;

/// Work shared by all expansions and probes in one logical query.
/// A probe and each examined cell candidate consume work, including candidates
/// subsequently rejected by distance or eligibility filters.
#[derive(Debug, Clone, Copy)]
pub struct QueryBudget {
    remaining: usize,
    used: usize,
    probes: usize,
}

impl QueryBudget {
    pub fn new(work: usize) -> Self {
        Self {
            remaining: work,
            used: 0,
            probes: 0,
        }
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn probes(&self) -> usize {
        self.probes
    }

    pub fn charge_probe(&mut self) -> Result<(), QueryExhausted> {
        self.charge()?;
        self.probes += 1;
        Ok(())
    }

    pub fn charge(&mut self) -> Result<(), QueryExhausted> {
        if self.remaining == 0 {
            return Err(QueryExhausted);
        }
        self.remaining -= 1;
        self.used += 1;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryExhausted;

impl fmt::Display for QueryExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("spatial query work budget exhausted")
    }
}

impl std::error::Error for QueryExhausted {}

#[derive(Debug)]
pub struct Nearest<'a, T> {
    pub item: &'a T,
    /// Exact caller-supplied distance, possibly negative inside an object.
    pub distance: f64,
}
