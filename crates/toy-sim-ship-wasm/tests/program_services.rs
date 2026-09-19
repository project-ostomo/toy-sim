use std::sync::{Arc, Mutex};
use toy_sim_model::{
    chat::{ChatMessage, ChatPage},
    firmware::{ChatterProfile, ProgramMemory},
    llm::{LlmRequest, LlmStatus, LlmSubmission},
};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{
    Command, Controller, ControllerCheckpoint, ControllerRuntime, FUEL_PER_TICK, Input,
    Observation, ProgramServices, Request,
};

#[derive(Default)]
struct State {
    requests: Vec<LlmRequest>,
    ready: Option<String>,
    sent: Vec<(u64, String)>,
    reads: usize,
}

#[derive(Default)]
struct Fake(Mutex<State>);

impl ProgramServices for Fake {
    fn llm_submit(&self, request: LlmRequest) -> LlmSubmission {
        let mut state = self.0.lock().unwrap();
        if state.requests.iter().any(|known| known.id == request.id) {
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
            .ready
            .clone()
            .map_or(LlmStatus::Pending, |text| LlmStatus::Ready { text })
    }

    fn llm_cancel(&self, _: u64) -> bool {
        false
    }

    fn chat_send(&self, id: u64, text: &str) -> Result<(), i32> {
        let mut state = self.0.lock().unwrap();
        if !state.sent.iter().any(|(known, _)| *known == id) {
            state.sent.push((id, text.to_owned()));
        }
        Ok(())
    }

    fn chat_send_work(&self) -> u64 {
        1_000
    }

    fn chat_read(&self, after: u64, _: u32) -> Result<ChatPage, i32> {
        self.0.lock().unwrap().reads += 1;
        Ok(ChatPage {
            messages: if after == 0 {
                vec![ChatMessage {
                    id: toy_sim_model::Id([7; 16]),
                    sequence: 1,
                    tick: 1,
                    calendar_unix_ms: 0,
                    sender_name: "Local pilot".into(),
                    advertised_owner: None,
                    advertised_organization: None,
                    text: "How is the anchorage today?".into(),
                }]
            } else {
                Vec::new()
            },
            next_sequence: 1,
            missed: 0,
        })
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

fn guest(body: &str, capacity: u32) -> Vec<u8> {
    let bytes = postcard::to_stdvec(&LlmRequest {
        id: 1,
        prompt: "A public radio greeting".into(),
        max_tokens: 32,
    })
    .unwrap();
    let data: String = bytes.iter().map(|byte| format!("\\{byte:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
        (import "ship_v29" "llm_submit" (func $submit (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 2)
        (data (i32.const 0) "{data}")
        (func (export "ship_api_version") (result i32) i32.const {version})
        (func (export "ship_tick")
            i32.const 0 i32.const {length} i32.const 65536 i32.const {capacity}
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
        .instantiate(&guest("i32.const 1 i32.ne if unreachable end", 1))
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
fn invalid_submit_output_buffer_never_admits_a_request() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .instantiate(&guest("i32.const -2 i32.ne if unreachable end", 0))
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
        (import "ship_v29" "llm_poll" (func $poll (param i64 i32 i32) (result i32)))
        (memory (export "memory") 2)
        (func (export "ship_api_version") (result i32) i32.const {version})
        (func $read i64.const 1 i32.const 0 i32.const 65568 call $poll drop)
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
    service.0.lock().unwrap().ready = Some("x".repeat(8_000));
    computer
        .run_callback_slice(
            toy_sim_ship_wasm::CallbackKind::Missile(7),
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
            program: include_bytes!("../../toy-sim-ships/data/chatter-controller.wasm").to_vec(),
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
    service.0.lock().unwrap().ready =
        Some("The instruments are ready for your next survey.".into());
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
    assert!(state.reads <= 2, "chatter polled chat every tick");
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
            program: toy_sim_ships::CHATTER_CONTROLLER.to_vec(),
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
