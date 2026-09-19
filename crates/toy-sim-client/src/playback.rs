use anyhow::{Result, ensure};
use std::{collections::VecDeque, sync::Arc};
use toy_sim_model::*;

pub(crate) struct Playback {
    frames: VecDeque<Arc<Frame>>,
    initial_target: usize,
    pub target_frames: usize,
    pub underruns: u64,
    pub(crate) world: Option<Id>,
    last_sequence: u64,
    playing: bool,
    current: Option<Arc<Frame>>,
    catching_up: bool,
    publications: VecDeque<Publications>,
}

#[derive(Default)]
pub(crate) struct Publications {
    pub sequence: u64,
    pub results: Vec<CommandResult>,
    pub events: Vec<Event>,
    pub combat: Vec<CombatEvent>,
}

impl Playback {
    pub fn new(local: bool) -> Self {
        let initial_target = if local { 1 } else { 3 };
        Self {
            frames: VecDeque::new(),
            initial_target,
            target_frames: initial_target,
            underruns: 0,
            world: None,
            last_sequence: 0,
            playing: false,
            current: None,
            catching_up: false,
            publications: VecDeque::new(),
        }
    }

    pub fn receive(&mut self, frame: impl Into<Arc<Frame>>) -> Result<()> {
        let frame = frame.into();
        if self.world != Some(frame.world) {
            self.frames.clear();
            self.current = None;
            self.playing = false;
            self.target_frames = self.initial_target;
            self.underruns = 0;
            self.last_sequence = 0;
            self.catching_up = false;
            self.publications.clear();
            self.world = Some(frame.world);
        }
        ensure!(
            frame.sequence > self.last_sequence,
            "out of order state sequence"
        );
        ensure!(
            self.frames
                .back()
                .or(self.current.as_ref())
                .is_none_or(|old| frame.sim_time_ns >= old.sim_time_ns),
            "simulation time moved backwards"
        );
        self.last_sequence = frame.sequence;
        self.frames.push_back(frame);
        Ok(())
    }

    fn publish(&mut self, frame: &Frame) {
        let results = frame.results.clone();

        let combat = frame.presentation.combat.clone();
        if !results.is_empty() || !combat.is_empty() || !frame.events.is_empty() {
            self.publications.push_back(Publications {
                sequence: frame.sequence,
                results,
                events: frame.events.clone(),
                combat,
            });
        }
    }

    pub fn tick(&mut self) -> Option<&Frame> {
        if !self.playing {
            if self.frames.len() < self.target_frames {
                return None;
            }
            self.playing = true;
        }
        if self.frames.len() > self.target_frames + 6 {
            self.catching_up = true;
        }
        if self.catching_up && self.frames.len() > self.target_frames {
            let frame = self.frames.pop_front().unwrap();
            self.publish(&frame);
            self.current = Some(frame);
        }
        if self.frames.len() <= self.target_frames {
            self.catching_up = false;
        }
        match self.frames.pop_front() {
            Some(frame) => {
                self.publish(&frame);
                self.current = Some(frame);
                self.current.as_deref()
            }
            None => {
                self.playing = false;
                self.underruns += 1;
                self.target_frames = (self.target_frames + 1).min(10);
                None
            }
        }
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    pub fn buffered_ns(&self) -> u64 {
        let Some(latest) = self.frames.back() else {
            return 0;
        };
        let earliest = self.current.as_ref().or(self.frames.front()).unwrap();
        latest.sim_time_ns.saturating_sub(earliest.sim_time_ns)
    }

    #[cfg(feature = "ui")]
    pub fn buffering(&self) -> bool {
        !self.playing
    }

    #[cfg(any(feature = "ui", test))]
    pub fn catching_up(&self) -> bool {
        self.catching_up
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.current.as_deref()
    }

    pub fn take_publications(&mut self, sequence: u64) -> Publications {
        let mut ready = Publications {
            sequence,
            ..Default::default()
        };
        while self
            .publications
            .front()
            .is_some_and(|batch| batch.sequence <= sequence)
        {
            let batch = self.publications.pop_front().unwrap();
            ready.results.extend(batch.results);
            ready.events.extend(batch.events);
            ready.combat.extend(batch.combat);
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn frame(sequence: u64) -> Frame {
        Frame {
            optical: Vec::new(),
            calendar_unix_ms: 0,
            society: Default::default(),
            world: Id([1; 16]),
            sequence,
            tick: sequence,
            sim_time_ns: sequence * 100_000_000,
            rate: 1.,
            views: Vec::new(),
            tracks: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
            presentation: PresentationFrame::default(),
        }
    }

    #[test]
    fn buffered_duration_uses_simulation_timestamps_including_pauses() {
        let mut playback = Playback::new(true);
        playback.receive(frame(1)).unwrap();
        playback.tick().unwrap();
        let mut next = frame(2);
        next.sim_time_ns = 450_000_000;
        playback.receive(next).unwrap();
        let mut paused = frame(3);
        paused.sim_time_ns = 450_000_000;
        playback.receive(paused).unwrap();
        assert_eq!(playback.queued_frames(), 2);
        assert!(!playback.catching_up());
        assert_eq!(playback.buffered_ns(), 350_000_000);
        playback.tick().unwrap();
        assert_eq!(playback.queued_frames(), 1);
        assert_eq!(playback.buffered_ns(), 0);
        playback.tick().unwrap();
        assert_eq!(playback.buffered_ns(), 0);
    }

    #[test]
    fn reserves_then_consumes_one_received_snapshot_per_tick() {
        let mut playback = Playback::new(false);
        playback.receive(frame(1)).unwrap();
        assert!(playback.tick().is_none());
        playback.receive(frame(2)).unwrap();
        assert!(playback.tick().is_none());
        assert_eq!(playback.underruns, 0);
        playback.receive(frame(3)).unwrap();
        for sequence in 1..=3 {
            assert_eq!(playback.tick().unwrap().sequence, sequence);
        }
        assert!(playback.tick().is_none());
        assert_eq!(playback.frame().unwrap().sequence, 3);
        assert_eq!(playback.target_frames, 4);
        assert_eq!(playback.underruns, 1);
    }

    #[test]
    fn depletion_counts_once_and_refill_requires_the_new_reserve() {
        let mut playback = Playback::new(true);
        playback.receive(frame(1)).unwrap();
        assert_eq!(playback.tick().unwrap().sequence, 1);
        for _ in 0..20 {
            assert!(playback.tick().is_none());
        }
        assert_eq!(playback.underruns, 1);
        assert_eq!(playback.target_frames, 2);
        playback.receive(frame(2)).unwrap();
        assert!(playback.tick().is_none());
        assert_eq!(playback.frame().unwrap().sequence, 1);
        playback.receive(frame(3)).unwrap();
        assert_eq!(playback.tick().unwrap().sequence, 2);
        assert_eq!(playback.tick().unwrap().sequence, 3);
        assert!(playback.tick().is_none());
        assert_eq!(playback.underruns, 2);
    }

    #[test]
    fn adaptive_reserve_stays_bounded_across_depletions() {
        let mut playback = Playback::new(true);
        let mut sequence = 0;
        for episode in 1..=15 {
            let reserve = playback.target_frames;
            for _ in 0..reserve {
                sequence += 1;
                playback.receive(frame(sequence)).unwrap();
            }
            for _ in 0..reserve {
                assert!(playback.tick().is_some());
            }
            assert!(playback.tick().is_none());
            assert_eq!(playback.underruns, episode);
            assert!(playback.target_frames <= 10);
        }
        assert_eq!(playback.target_frames, 10);
    }

    #[test]
    fn large_backlog_is_consumed_in_order_with_catchup_hysteresis() {
        let mut playback = Playback::new(false);
        for sequence in 1..=1000 {
            let mut snapshot = frame(sequence);
            snapshot.results.push(CommandResult {
                id: Id((sequence as u128).to_le_bytes()),
                effective_tick: sequence,
                error: None,
                reply: None,
            });
            snapshot.presentation.combat.push(CombatEvent {
                sequence,
                sim_time_ns: snapshot.sim_time_ns,
                kind: CombatEventKind::Fired {
                    source: ContactRef {
                        group: Id([2; 16]),
                        track: Id([3; 16]),
                    },
                    position: GalacticPosition::ZERO,
                    energy_j: 1.,
                },
            });
            playback.receive(snapshot).unwrap();
        }
        assert_eq!(playback.frames.len(), 1000);
        for sequence in (2..=998).step_by(2) {
            assert_eq!(playback.tick().unwrap().sequence, sequence);
            let publications = playback.take_publications(sequence);
            assert_eq!(publications.results.len(), 2);
            assert_eq!(publications.results[0].effective_tick, sequence - 1);
            assert_eq!(publications.results[1].effective_tick, sequence);
            assert_eq!(publications.combat.len(), 2);
            assert_eq!(publications.combat[0].sequence, sequence - 1);
            assert_eq!(publications.combat[1].sequence, sequence);
        }
        assert!(!playback.catching_up);
        assert_eq!(playback.tick().unwrap().sequence, 999);
        assert_eq!(playback.tick().unwrap().sequence, 1000);
        assert_eq!(playback.underruns, 0);
    }

    #[test]
    fn actual_simulation_timestamps_survive_skips_speed_changes_and_pause() {
        let mut playback = Playback::new(true);
        playback.receive(frame(1)).unwrap();
        assert_eq!(playback.tick().unwrap().sim_time_ns, 100_000_000);
        playback.receive(frame(4)).unwrap();
        assert_eq!(playback.tick().unwrap().sim_time_ns, 400_000_000);
        let mut accelerated = frame(5);
        accelerated.sim_time_ns = 50_000_000_000;
        accelerated.tick = 500;
        accelerated.rate = 100.;
        playback.receive(accelerated.clone()).unwrap();
        assert_eq!(playback.tick(), Some(&accelerated));
        accelerated.sequence = 6;
        accelerated.rate = 0.;
        playback.receive(accelerated.clone()).unwrap();
        assert_eq!(playback.tick(), Some(&accelerated));
        assert_eq!(playback.underruns, 0);
    }

    #[test]
    fn invalid_order_is_rejected_and_new_world_clears_playback_state() {
        let mut playback = Playback::new(false);
        let command = Id([8; 16]);
        for sequence in 1..=3 {
            let mut snapshot = frame(sequence);
            snapshot.events.push(Event {
                sequence: 1,
                tick: 1,
                subject: None,
                kind: "previous world".into(),
                position: None,
            });
            snapshot.results.push(CommandResult {
                id: command,
                effective_tick: 1,
                error: None,
                reply: None,
            });
            playback.receive(snapshot).unwrap();
        }
        assert!(playback.receive(frame(3)).is_err());
        let mut backwards = frame(4);
        backwards.sim_time_ns = 1;
        assert!(playback.receive(backwards).is_err());
        assert_eq!(playback.last_sequence, 3);
        for _ in 0..3 {
            playback.tick().unwrap();
        }
        assert!(playback.tick().is_none());
        assert_eq!(playback.target_frames, 4);
        let mut reset = frame(1);
        reset.world = Id([2; 16]);
        playback.receive(reset).unwrap();
        assert!(playback.frame().is_none());
        assert_eq!(playback.target_frames, 3);
        assert_eq!(playback.underruns, 0);
        assert_eq!(playback.last_sequence, 1);
        assert!(playback.take_publications(1).results.is_empty());
        assert!(playback.tick().is_none());
    }
}
