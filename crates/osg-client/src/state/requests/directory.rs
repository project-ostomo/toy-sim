use osg_model::ownership::Principal;
pub use osg_model::society::Branch;
use osg_model::society::SocietyPresentation;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default)]
pub struct Status {
    pub loaded: bool,
    pub error: Option<String>,
}

/// UI loading metadata for the current response; identity records live only in
/// that response and are replaced together.
#[derive(Default)]
pub struct State {
    branches: BTreeMap<Branch, Status>,
    pub matches: BTreeSet<Principal>,
    pub search_status: Status,
}

impl State {
    pub fn loading(&self) -> bool {
        self.branches.is_empty()
    }

    pub fn statuses(&self) -> BTreeMap<Branch, Status> {
        self.branches.clone()
    }

    pub fn replace(&mut self, view: &SocietyPresentation) {
        self.branches = view
            .branches
            .iter()
            .map(|branch| {
                (
                    *branch,
                    Status {
                        loaded: true,
                        error: None,
                    },
                )
            })
            .collect();
        self.matches = view.matches.clone();
        self.search_status = Status {
            loaded: true,
            error: None,
        };
    }
}
