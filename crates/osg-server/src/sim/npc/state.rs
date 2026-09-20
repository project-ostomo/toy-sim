use anyhow::{Result, ensure};
use bevy::prelude::Component;
use osg_model::{Id, llm::LlmRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_ASSETS: usize = 128;
pub const MAX_TOOL_RESULTS: usize = 8;
pub const MAX_TOOL_RESULT_BYTES: usize = 16 * 1024;
pub const MAX_QUERY_ROUNDS: u8 = 2;

pub fn director_program() -> [u8; 32] {
    *blake3::hash(b"OpenSpaceGame organization director version 1").as_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NpcRole {
    Station,
    Freighter,
    Patrol,
    Miner,
    Research,
    Broadcast,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpcAsset {
    pub id: Id,
    pub role: NpcRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDecision {
    pub request: LlmRequest,
    pub submitted_tick: u64,
    pub revision: u64,
}

#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpcOrganization {
    pub organization: Id,
    pub officer: Id,
    pub home_system: Id,
    pub facility: Id,
    pub assets: Vec<NpcAsset>,
    pub director_program: [u8; 32],
    pub next_request_id: u64,
    pub pending: Option<PendingDecision>,
    pub next_think_tick: u64,
    pub last_decision_id: u64,
    pub last_action_id: u64,
    pub revision: u64,
    pub tool_results: Vec<String>,
    pub query_rounds: u8,
}

impl NpcOrganization {
    pub fn new(
        organization: Id,
        officer: Id,
        home_system: Id,
        facility: Id,
        assets: Vec<NpcAsset>,
        next_think_tick: u64,
    ) -> Self {
        Self {
            organization,
            officer,
            home_system,
            facility,
            assets,
            director_program: director_program(),
            next_request_id: 1,
            pending: None,
            next_think_tick,
            last_decision_id: 0,
            last_action_id: 0,
            revision: 0,
            tool_results: Vec::new(),
            query_rounds: 0,
        }
    }

    pub fn validate(&self, current_tick: u64) -> Result<()> {
        ensure!(
            !self.assets.is_empty() && self.assets.len() <= MAX_ASSETS,
            "invalid NPC asset roster size"
        );
        let ids: BTreeSet<_> = self.assets.iter().map(|asset| asset.id).collect();
        ensure!(
            ids.len() == self.assets.len() && !ids.contains(&Id::default()),
            "duplicate or invalid NPC asset assignment"
        );
        ensure!(
            self.assets
                .iter()
                .any(|asset| asset.id == self.facility && asset.role == NpcRole::Station),
            "NPC primary facility is missing from its station roster"
        );
        ensure!(
            self.director_program == director_program(),
            "unsupported saved NPC director version"
        );
        ensure!(
            self.next_request_id > 0 && self.last_decision_id < self.next_request_id,
            "invalid NPC decision sequence"
        );
        ensure!(
            self.tool_results.len() <= MAX_TOOL_RESULTS
                && self.query_rounds <= MAX_QUERY_ROUNDS
                && self.tool_results.iter().map(String::len).sum::<usize>()
                    <= MAX_TOOL_RESULT_BYTES,
            "NPC tool results exceed their durable limit"
        );

        if let Some(pending) = &self.pending {
            ensure!(
                pending.request.valid()
                    && pending.request.id > self.last_decision_id
                    && pending.request.id < self.next_request_id
                    && pending.submitted_tick <= current_tick
                    && pending.revision <= self.revision,
                "invalid pending NPC planning request"
            );
        }
        Ok(())
    }
}
