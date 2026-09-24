//! Ordinary authenticated clients for external officers, traders and other agents.
use crate::{EventSubscription, NetEvent, OsgNetClient, Outgoing};
use anyhow::{Context, Result, ensure};
use osg_model::{CommandResult, Frame, Id};
use std::{collections::BTreeMap, sync::Arc};

pub struct HeadlessClient {
    pub client: OsgNetClient,
    events: EventSubscription,
    pub outgoing: Outgoing,
    pub latest: Option<Arc<Frame>>,
    results: BTreeMap<Id, CommandResult>,
}

impl HeadlessClient {
    pub fn new(client: OsgNetClient) -> Self {
        Self {
            events: client.subscribe_events(),
            client,
            outgoing: Outgoing::default(),
            latest: None,
            results: BTreeMap::new(),
        }
    }

    /// Receive authoritative state and correlate command results. A new world
    /// invalidates queued commands and results referring to the previous world.
    pub async fn receive(&mut self) -> Result<Arc<Frame>> {
        // Apply backpressure before removing a frame from the transport. Retain
        // all results in an accepted frame, including a batch crossing the limit.
        ensure!(
            self.results.len() < 4096,
            "consume command results before receiving more"
        );
        let frame = loop {
            if let NetEvent::Frame(frame) = self
                .events
                .recv()
                .await
                .context("main stream closed or lagged; reconnect")?
            {
                break frame;
            }
        };
        if self
            .latest
            .as_ref()
            .is_some_and(|previous| previous.world != frame.world)
        {
            self.outgoing.clear();
            self.results.clear();
        }
        for result in &frame.results {
            self.results.insert(result.id, result.clone());
        }
        self.latest = Some(frame.clone());
        Ok(frame)
    }

    pub fn take_result(&mut self, id: Id) -> Option<CommandResult> {
        self.results.remove(&id)
    }

    /// IDs survive backpressure. Each batch uses the same typed actions as the UI.
    pub async fn flush(&mut self) -> Result<()> {
        let world = self
            .latest
            .as_ref()
            .context("receive initial world state first")?
            .world;
        while !self.outgoing.pending().is_empty() {
            let count = self.outgoing.pending().len().min(256);
            self.client
                .send_inputs(world, &self.outgoing.pending()[..count])
                .await?;
            self.outgoing.take();
        }
        Ok(())
    }
}
