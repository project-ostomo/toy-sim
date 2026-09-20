use osg_model::{
    chat::{ChatMessage, ChatPage, MAX_HISTORY_MESSAGES, MAX_PAGE_MESSAGES},
    firmware::{ChatterProfile, ProgramMemory},
    llm::{LlmRequest, LlmStatus, LlmSubmission, MAX_PROMPT_BYTES},
};
use osg_ship_api::abi;
use osg_ship_wasm::{
    Command, Controller, ControllerCheckpoint, ControllerRuntime, FUEL_PER_TICK, Input,
    Observation, ProgramServices, Request,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Debug)]
struct Read {
    after: u64,
    limit: u32,
    page: ChatPage,
}

#[derive(Default)]
struct State {
    requests: Vec<LlmRequest>,
    statuses: BTreeMap<u64, LlmStatus>,
    sent: Vec<(u64, String)>,
    send_attempts: Vec<(u64, String)>,
    inbox: VecDeque<ChatMessage>,
    sequence: u64,
    reads: Vec<Read>,
}

struct Fake(Mutex<State>);

impl Default for Fake {
    fn default() -> Self {
        let fake = Self(Mutex::new(State::default()));
        fake.receive("Local pilot", "How is the anchorage today?");
        fake
    }
}

impl Fake {
    fn receive(&self, sender: &str, text: &str) -> u64 {
        assert!(osg_model::chat::valid_text(text));
        assert!(sender.len() <= osg_model::chat::MAX_SENDER_BYTES);
        let mut state = self.0.lock().unwrap();
        state.sequence += 1;
        let sequence = state.sequence;
        state.inbox.push_back(ChatMessage {
            id: osg_model::Id(u128::from(sequence).to_le_bytes()),
            sequence,
            tick: sequence,
            calendar_unix_ms: sequence as i64 * 100,
            sender_name: sender.to_owned(),
            advertised_owner: None,
            advertised_organization: None,
            text: text.to_owned(),
        });
        while state.inbox.len() > MAX_HISTORY_MESSAGES {
            state.inbox.pop_front();
        }
        sequence
    }

    fn ready(&self, id: u64, text: &str) {
        self.0
            .lock()
            .unwrap()
            .statuses
            .insert(id, LlmStatus::Ready { text: text.into() });
    }
}

impl ProgramServices for Fake {
    fn llm_submit(&self, request: LlmRequest) -> LlmSubmission {
        if !request.valid() {
            return LlmSubmission::InvalidRequest;
        }
        let mut state = self.0.lock().unwrap();
        if let Some(known) = state.requests.iter().find(|known| known.id == request.id) {
            assert_eq!(known, &request, "pending request changed during retry");
            return LlmSubmission::AlreadyKnown;
        }
        state.requests.push(request);
        LlmSubmission::Accepted
    }

    fn llm_poll(&self, id: u64) -> LlmStatus {
        let state = self.0.lock().unwrap();
        if !state.requests.iter().any(|request| request.id == id) {
            return LlmStatus::Unknown;
        }
        state
            .statuses
            .get(&id)
            .cloned()
            .unwrap_or(LlmStatus::Pending)
    }

    fn llm_cancel(&self, _: u64) -> bool {
        false
    }

    fn chat_send(&self, id: u64, text: &str) -> Result<(), i32> {
        let mut state = self.0.lock().unwrap();
        state.send_attempts.push((id, text.to_owned()));
        if !state.sent.iter().any(|(known, _)| *known == id) {
            state.sent.push((id, text.to_owned()));
        }
        Ok(())
    }

    fn chat_send_work(&self) -> u64 {
        1_000
    }

    fn chat_read(&self, after: u64, limit: u32) -> Result<ChatPage, i32> {
        let mut state = self.0.lock().unwrap();
        if !(1..=MAX_PAGE_MESSAGES as u32).contains(&limit) || after > state.sequence {
            return Err(abi::ERR_ARGUMENT);
        }
        let missed = state.inbox.front().map_or(0, |first| {
            first.sequence.saturating_sub(after.saturating_add(1))
        });
        let messages: Vec<_> = state
            .inbox
            .iter()
            .filter(|message| message.sequence > after)
            .take(limit as usize)
            .cloned()
            .collect();
        let page = ChatPage {
            next_sequence: messages.last().map_or(after, |message| message.sequence),
            messages,
            missed,
        };
        state.reads.push(Read {
            after,
            limit,
            page: page.clone(),
        });
        Ok(page)
    }
}

fn input(tick: u64) -> Input {
    Input {
        tick,
        observation: Observation {
            time_s: tick as f64 * 0.1,
            flight: abi::FlightState {
                rotation: [0., 0., 0., 1.],
                mass_kg: 1_000.,
                inertia: [100., 0., 0., 0., 100., 0., 0., 0., 100.],
                radius_m: 2.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn boot(computer: &mut Controller) {
    for _ in 0..80 {
        if !computer.is_booting() {
            return;
        }
        computer
            .run_slice(input(0), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    panic!("computer boot did not finish");
}

fn guest(body: &str, max_tokens: u32) -> Vec<u8> {
    let bytes = b"A public radio greeting";
    let data: String = bytes.iter().map(|byte| format!("\\{byte:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
        (import "ship_v32" "llm_submit" (func $submit (param i64 i32 i32 i32) (result i32)))
        (memory (export "memory") 2)
        (data (i32.const 0) "{data}")
        (func (export "ship_api_version") (result i32) i32.const {version})
        (func (export "ship_tick")
            i64.const 1 i32.const 0 i32.const {length} i32.const {max_tokens}
            call $submit {body}))"#,
        version = abi::VERSION,
        length = bytes.len(),
    ))
    .unwrap()
}

#[test]
fn request_side_effect_waits_for_paid_admission_and_uses_fresh_resume_authority() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .instantiate(&guest("i32.const 0 i32.ne if unreachable end", 32))
        .unwrap();
    boot(&mut computer);
    let old = Arc::new(Fake::default());
    let current = Arc::new(Fake::default());
    computer.set_services(Some(old.clone()));
    let slice = computer
        .run_slice(input(1), None, 1_000, FUEL_PER_TICK)
        .unwrap();
    assert!(!slice.callback_completed);
    assert!(computer.is_suspended());
    assert!(old.0.lock().unwrap().requests.is_empty());

    computer.set_services(Some(current.clone()));
    let slice = computer
        .run_slice(input(2), None, 100_000, FUEL_PER_TICK)
        .unwrap();
    assert!(slice.callback_completed);
    assert!(old.0.lock().unwrap().requests.is_empty());
    assert_eq!(current.0.lock().unwrap().requests.len(), 1);
    assert!(computer.last_gas_used < 100_000);
}

#[test]
fn invalid_submit_request_never_admits_a_request() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .instantiate(&guest("i32.const -3 i32.ne if unreachable end", 0))
        .unwrap();
    boot(&mut computer);
    let service = Arc::new(Fake::default());
    computer.set_services(Some(service.clone()));
    let slice = computer
        .run_slice(input(1), None, 100_000, FUEL_PER_TICK)
        .unwrap();
    assert!(slice.callback_completed);
    assert!(service.0.lock().unwrap().requests.is_empty());
}

#[test]
fn poll_cpu_gas_tracks_reply_bytes_and_missile_and_display_callbacks_can_call_services() {
    let program = wat::parse_str(format!(
        r#"(module
        (import "ship_v32" "llm_poll" (func $poll (param i64 i32 i32 i32) (result i32)))
        (memory (export "memory") 2)
        (func (export "ship_api_version") (result i32) i32.const {version})
        (func $read i64.const 1 i32.const 0 i32.const 65568 i32.const 70000 call $poll drop)
        (func (export "ship_tick") call $read)
        (func (export "ship_display") call $read)
        (func (export "missile_tick") (param i64) call $read))"#,
        version = abi::VERSION,
    ))
    .unwrap();
    let service = Arc::new(Fake::default());
    service.0.lock().unwrap().requests.push(LlmRequest {
        id: 1,
        prompt: "Known request".into(),
        max_tokens: 32,
    });
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(&program).unwrap();
    boot(&mut computer);
    computer.set_services(Some(service.clone()));
    computer
        .run_slice(input(1), None, 100_000, FUEL_PER_TICK)
        .unwrap();
    let short = computer.last_gas_used;
    service.ready(1, &"x".repeat(8_000));
    computer
        .run_callback_slice(
            osg_ship_wasm::CallbackKind::Missile(7),
            input(2),
            None,
            Some(abi::MissileObservation {
                handle: 7,
                ..Default::default()
            }),
            100_000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(computer.last_gas_used >= short + 1_000);

    let mut display = runtime.instantiate_display(&program).unwrap();
    boot(&mut display);
    display.set_services(Some(service));
    assert!(
        display
            .run_slice(input(3), None, 100_000, FUEL_PER_TICK)
            .unwrap()
            .callback_completed
    );
}

#[test]
fn chatter_uses_delayed_llm_result_survives_restart_and_preserves_flight_memory() {
    let memory = ProgramMemory {
        flight: b"existing-flight-state".to_vec(),
        chatter: Some(ChatterProfile {
            name: "Research tender".into(),
            personality: "Patient and observant".into(),
            context: "Maintains public instruments near Neris anchorage".into(),
            interval_seconds: 60,
        }),
        chatter_state: Vec::new(),
    };
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .restore(&ControllerCheckpoint {
            program: include_bytes!("../../osg-ships/data/chatter-controller.wasm").to_vec(),
            persistent_data: postcard::to_stdvec(&memory).unwrap(),
        })
        .unwrap();
    boot(&mut computer);
    let service = Arc::new(Fake::default());
    computer.set_services(Some(service.clone()));
    let mut accepted = false;
    for tick in 1..160 {
        let mut input = input(tick);
        if tick == 1 {
            input.commands.push(Request {
                id: 11,
                command: Command::Manual {
                    throttle: 0.0,
                    steering: [0.0; 3],
                },
            });
        }
        let slice = computer
            .run_slice(input, None, FUEL_PER_TICK / 2, FUEL_PER_TICK)
            .unwrap();
        accepted |= slice
            .output
            .replies
            .iter()
            .any(|reply| reply.id == 11 && reply.result == abi::REPLY_ACCEPTED);
    }
    assert!(
        accepted,
        "flight requests did not progress alongside chatter"
    );
    {
        let state = service.0.lock().unwrap();
        assert_eq!(state.requests.len(), 1);
        assert!(
            state.requests[0]
                .prompt
                .contains("How is the anchorage today?")
        );
        assert!(state.sent.is_empty(), "broadcast without a provider result");
    }
    let saved = computer.checkpoint();
    let envelope: ProgramMemory = postcard::from_bytes(&saved.persistent_data).unwrap();
    assert_eq!(envelope.flight, memory.flight);
    assert!(!envelope.chatter_state.is_empty());

    let mut computer = runtime.restore(&saved).unwrap();
    boot(&mut computer);
    computer.set_services(Some(service.clone()));
    service.ready(1, "The instruments are ready for your next survey.");
    for tick in 160..260 {
        computer
            .run_slice(input(tick), None, FUEL_PER_TICK / 2, FUEL_PER_TICK)
            .unwrap();
    }
    let state = service.0.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    assert_eq!(
        state.sent,
        vec![(1, "The instruments are ready for your next survey.".into())]
    );
    assert!(
        (8..=16).contains(&state.reads.len()),
        "unexpected intake cadence: {} reads",
        state.reads.len()
    );
    assert_eq!(state.send_attempts, state.sent);
    let envelope: ProgramMemory =
        postcard::from_bytes(&computer.checkpoint().persistent_data).unwrap();
    assert_eq!(envelope.flight, memory.flight);
}

#[test]
fn chatter_does_not_submit_until_its_request_id_fits_in_durable_storage() {
    let memory = ProgramMemory {
        flight: vec![9; 64_000],
        chatter: Some(ChatterProfile {
            name: "Research tender".into(),
            personality: "Patient".into(),
            context: "x".repeat(1_000),
            interval_seconds: 60,
        }),
        chatter_state: Vec::new(),
    };
    let bytes = postcard::to_stdvec(&memory).unwrap();
    assert!(bytes.len() < 65_536);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .restore(&ControllerCheckpoint {
            program: osg_ships::CHATTER_CONTROLLER.to_vec(),
            persistent_data: bytes.clone(),
        })
        .unwrap();
    boot(&mut computer);
    let service = Arc::new(Fake::default());
    computer.set_services(Some(service.clone()));
    for tick in 1..160 {
        computer
            .run_slice(input(tick), None, FUEL_PER_TICK / 2, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(service.0.lock().unwrap().requests.is_empty());
    assert!(service.0.lock().unwrap().sent.is_empty());
    assert_eq!(computer.checkpoint().persistent_data, bytes);
}

fn chatter_tick(computer: &mut Controller, tick: &mut u64) -> bool {
    *tick += 1;
    computer
        .run_slice(input(*tick), None, FUEL_PER_TICK / 2, FUEL_PER_TICK)
        .unwrap()
        .callback_completed
}

fn saved_memory(computer: &Controller, flight: &[u8]) -> ProgramMemory {
    let saved = computer.checkpoint();
    assert!(saved.persistent_data.len() <= 65_536);
    let memory: ProgramMemory = postcard::from_bytes(&saved.persistent_data).unwrap();
    assert_eq!(memory.flight, flight);
    memory
}

#[test]
fn chatter_drains_busy_radio_during_pending_reply_and_resumes_after_checkpoint() {
    let memory = ProgramMemory {
        flight: b"flight-state-during-radio-backlog".to_vec(),
        chatter: Some(ChatterProfile {
            name: "Anchorage watch".into(),
            personality: "Patient and precise".into(),
            context: "Answers local pilots using received public radio messages".into(),
            interval_seconds: 300,
        }),
        chatter_state: Vec::new(),
    };
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .restore(&ControllerCheckpoint {
            program: osg_ships::CHATTER_CONTROLLER.to_vec(),
            persistent_data: postcard::to_stdvec(&memory).unwrap(),
        })
        .unwrap();
    boot(&mut computer);
    let service = Arc::new(Fake::default());
    computer.set_services(Some(service.clone()));
    let mut tick = 0;
    assert!(
        (0..200).any(|_| {
            let complete = chatter_tick(&mut computer, &mut tick);
            complete && service.0.lock().unwrap().requests.len() == 1
        }),
        "first paid request was never submitted"
    );

    let question = "Player question: which public berth handles the cobalt survey launch?";
    let mut latest = 0;
    for line in 1..=90 {
        latest = service.receive(
            if line == 85 {
                "Player surveyor"
            } else {
                "Passing pilot"
            },
            &if line == 85 {
                question.to_owned()
            } else {
                format!("Public traffic report number {line}")
            },
        );
    }
    assert!(
        (0..200).any(|_| {
            let complete = chatter_tick(&mut computer, &mut tick);
            let state = service.0.lock().unwrap();
            complete
                && state.reads.last().is_some_and(|read| {
                    read.page.messages.len() == MAX_PAGE_MESSAGES
                        && read.page.next_sequence > 1
                        && read.page.next_sequence < latest
                })
        }),
        "pending chatter did not begin draining the radio backlog"
    );
    let (cursor, previous_reads) = {
        let state = service.0.lock().unwrap();
        assert_eq!(state.requests.len(), 1);
        assert!(state.sent.is_empty());
        assert!(!state.requests[0].prompt.contains(question));
        (
            state.reads.last().unwrap().page.next_sequence,
            state.reads.len(),
        )
    };
    assert!(
        cursor < 85,
        "checkpoint must precede the late player question"
    );
    saved_memory(&computer, &memory.flight);
    let checkpoint = computer.checkpoint();
    let mut computer = runtime.restore(&checkpoint).unwrap();
    boot(&mut computer);
    computer.set_services(Some(service.clone()));

    assert!(
        (0..300).any(|_| {
            let complete = chatter_tick(&mut computer, &mut tick);
            let state = service.0.lock().unwrap();
            complete
                && state
                    .reads
                    .last()
                    .is_some_and(|read| read.page.next_sequence == latest)
        }),
        "restored chatter did not catch up while the provider stayed pending"
    );
    {
        let state = service.0.lock().unwrap();
        assert_eq!(state.reads[previous_reads].after, cursor);
        assert!(
            state
                .reads
                .iter()
                .all(|read| read.limit == MAX_PAGE_MESSAGES as u32)
        );
        assert!(
            state
                .reads
                .iter()
                .all(|read| read.page.messages.len() <= MAX_PAGE_MESSAGES)
        );
        assert_eq!(
            state.requests.len(),
            1,
            "intake created another paid request"
        );
        assert!(state.sent.is_empty());
    }
    saved_memory(&computer, &memory.flight);
    service.ready(1, "The first watch report is complete.");
    assert!(
        (0..200).any(|_| {
            chatter_tick(&mut computer, &mut tick);
            service.0.lock().unwrap().sent.len() == 1
        }),
        "completed provider result was never broadcast"
    );
    let sent_tick = tick;

    while tick < sent_tick + 2_990 {
        chatter_tick(&mut computer, &mut tick);
        assert_eq!(
            service.0.lock().unwrap().requests.len(),
            1,
            "paid cadence accelerated"
        );
    }
    assert!(
        (0..160).any(|_| {
            chatter_tick(&mut computer, &mut tick);
            service.0.lock().unwrap().requests.len() == 2
        }),
        "next paid cadence never submitted its updated context"
    );
    for _ in 0..40 {
        chatter_tick(&mut computer, &mut tick);
    }
    {
        let state = service.0.lock().unwrap();
        assert_eq!(
            state
                .requests
                .iter()
                .map(|request| request.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(state.requests[1].prompt.contains(question));
        assert!(
            state
                .requests
                .iter()
                .all(|request| request.prompt.len() <= MAX_PROMPT_BYTES)
        );
        assert_eq!(
            state.sent,
            vec![(1, "The first watch report is complete.".into())]
        );
        assert_eq!(
            state.send_attempts, state.sent,
            "a broadcast was retried unnecessarily"
        );
        assert!(
            state.reads.len() < tick as usize / 10,
            "radio intake ran every tick"
        );
    }
    saved_memory(&computer, &memory.flight);
}

#[test]
fn chatter_bounds_unicode_radio_context_and_maximum_profile_during_history_loss() {
    let profile = ChatterProfile {
        name: "船".repeat(42),
        personality: "界".repeat(1365),
        context: "🌌".repeat(2048),
        interval_seconds: 300,
    };
    assert!(profile.valid());
    let memory = ProgramMemory {
        flight: vec![23; 1_024],
        chatter: Some(profile),
        chatter_state: Vec::new(),
    };
    let service = Arc::new(Fake::default());
    let sender = "航".repeat(42);
    for line in 1..=160 {
        let prefix = format!("最新の公開通信 {line}: ");
        let text = format!("{prefix}{}", "🚀".repeat((1024 - prefix.len()) / 4));
        service.receive(&sender, &text);
    }
    assert_eq!(service.0.lock().unwrap().inbox.len(), MAX_HISTORY_MESSAGES);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .restore(&ControllerCheckpoint {
            program: osg_ships::CHATTER_CONTROLLER.to_vec(),
            persistent_data: postcard::to_stdvec(&memory).unwrap(),
        })
        .unwrap();
    boot(&mut computer);
    computer.set_services(Some(service.clone()));
    let mut tick = 0;
    assert!(
        (0..400).any(|_| {
            let complete = chatter_tick(&mut computer, &mut tick);
            if complete {
                saved_memory(&computer, &memory.flight);
            }
            !service.0.lock().unwrap().requests.is_empty()
        }),
        "bounded Unicode context did not produce a paid request"
    );
    let state = service.0.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    assert!(state.requests[0].valid());
    assert!(state.requests[0].prompt.len() <= 32 * 1024);
    assert!(state.requests[0].prompt.contains("最新の公開通信 160:"));
    assert!(state.reads[0].page.missed > 0);
    assert!(
        state
            .reads
            .iter()
            .all(|read| read.page.messages.len() <= MAX_PAGE_MESSAGES)
    );
    assert!(state.sent.is_empty());
    drop(state);
    saved_memory(&computer, &memory.flight);
}

#[test]
fn profile_installation_preserves_flight_and_pending_chatter_memory_atomically() {
    let profile = ChatterProfile {
        name: "Anchorage watch".into(),
        personality: "Reserved".into(),
        context: "A public station operator".into(),
        interval_seconds: 60,
    };
    let memory = ProgramMemory {
        flight: b"flight-state".to_vec(),
        chatter: None,
        chatter_state: b"pending-request-state".to_vec(),
    };
    let mut checkpoint = ControllerCheckpoint {
        program: osg_ships::CHATTER_CONTROLLER.to_vec(),
        persistent_data: postcard::to_stdvec(&memory).unwrap(),
    };
    checkpoint.install_chatter_profile(profile.clone()).unwrap();
    let updated: ProgramMemory = postcard::from_bytes(&checkpoint.persistent_data).unwrap();
    assert_eq!(updated.flight, memory.flight);
    assert_eq!(updated.chatter_state, memory.chatter_state);
    assert_eq!(updated.chatter, Some(profile.clone()));

    let before = checkpoint.persistent_data.clone();
    let mut invalid = profile.clone();
    invalid.interval_seconds = 0;
    assert!(checkpoint.install_chatter_profile(invalid).is_err());
    assert_eq!(checkpoint.persistent_data, before);

    let mut runtime = ControllerRuntime::new().unwrap();
    let controller = runtime
        .instantiate_with_chatter(osg_ships::CHATTER_CONTROLLER, profile)
        .unwrap();
    assert_eq!(controller.boot_remaining_gas(), osg_ship_wasm::BOOT_GAS);
    assert!(controller.is_booting());
}
