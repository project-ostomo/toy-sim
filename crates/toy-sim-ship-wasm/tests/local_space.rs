use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use toy_sim_model::{
    ContactRef, GalacticPosition, Id, LocalObstacle, LocalSpace, Pose, ProgramQuery, ProgramReply,
    local_space::{MAX_LOCAL_OBSTACLES, QUERY_GAS},
    travel::Target,
};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{ControllerRuntime, FUEL_PER_TICK, Input, ScanSource, SensorContact};

struct LocalSource {
    calls: AtomicUsize,
    reply: LocalSpace,
}

impl ScanSource for LocalSource {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(&self, query: ProgramQuery, _: bool, _: usize) -> anyhow::Result<ProgramReply> {
        let ProgramQuery::LocalSpace {
            range_m,
            after_seconds,
            ..
        } = query
        else {
            anyhow::bail!("unexpected query");
        };
        assert_eq!((range_m, after_seconds), (1e6, 10.));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProgramReply::LocalSpace(self.reply.clone()))
    }
}

#[test]
fn local_observation_query_suspends_before_work_and_copies_a_full_bounded_reply() {
    let request = ProgramQuery::LocalSpace {
        destination: GalacticPosition::ZERO,
        range_m: 1e6,
        after_seconds: 10.,
    };
    assert_eq!(toy_sim_ship_wasm::query_work(&request), QUERY_GAS);
    let source = Arc::new(LocalSource {
        calls: AtomicUsize::new(0),
        reply: LocalSpace {
            obstacles: (0..MAX_LOCAL_OBSTACLES)
                .map(|index| LocalObstacle {
                    reference: Target::Contact(ContactRef {
                        group: Id([2; 16]),
                        track: Id([index as u8; 16]),
                    }),
                    pose: Pose::default(),
                    radius_m: 10.,
                    slip_exclusion_m: 0.,
                })
                .collect(),
            truncated: true,
        },
    });
    let reply_bytes =
        postcard::to_allocvec(&ProgramReply::LocalSpace(source.reply.clone())).unwrap();
    assert!(reply_bytes.len() < 65536);
    let bytes = postcard::to_allocvec(&request).unwrap();
    let data: String = bytes.iter().map(|byte| format!("\\{byte:02x}")).collect();
    let program = wat::parse_str(format!(
        r#"(module
            (import "ship_v30" "world_query" (func $query (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 2)
            (data (i32.const 0) "{data}")
            (func (export "ship_api_version") (result i32) i32.const {version})
            (func (export "ship_tick")
                i32.const 0 i32.const {length} i32.const 4096 i32.const 65536 call $query
                i32.const {reply_length} i32.ne if unreachable end))"#,
        version = abi::VERSION,
        length = bytes.len(),
        reply_length = reply_bytes.len(),
    ))
    .unwrap();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(&program).unwrap();
    for _ in 0..60 {
        if !computer.is_booting() {
            break;
        }
        computer
            .run_slice(Input::default(), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(!computer.is_booting());

    let waiting = computer
        .run_slice(
            Input::default(),
            Some(source.clone()),
            200_000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(!waiting.callback_completed);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert!(computer.minimum_to_progress() > QUERY_GAS);
    assert!(computer.last_gas_used <= 200_000);

    let complete = computer
        .run_slice(
            Input::default(),
            Some(source.clone()),
            300_000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(complete.callback_completed);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(computer.last_gas_used >= QUERY_GAS);
    assert!(computer.last_gas_used <= 300_000);
    assert!(computer.fault.is_none());
}
