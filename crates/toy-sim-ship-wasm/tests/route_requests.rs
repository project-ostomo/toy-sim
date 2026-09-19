use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use toy_sim_model::{ProgramQuery, ProgramReply, routing, travel};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{ControllerRuntime, FUEL_PER_TICK, Input, ScanSource, SensorContact};

struct JobSource {
    calls: AtomicUsize,
}

impl ScanSource for JobSource {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(&self, query: ProgramQuery, _: bool, _: usize) -> anyhow::Result<ProgramReply> {
        let ProgramQuery::RouteRequest(request) = query else {
            anyhow::bail!("unexpected query");
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProgramReply::Route {
            id: request.id,
            status: routing::Status::Pending {
                progress: travel::PlanningProgress {
                    stage: travel::PlanningStage::SearchingRoutes,
                    completed: 0,
                    total: None,
                },
            },
        })
    }
}

#[test]
fn route_request_prepays_bounded_admission_and_resumes_exactly_once() {
    let request = ProgramQuery::RouteRequest(routing::Request {
        id: 42,
        orders: vec![travel::Order::WaitUntil(10)],
        preferences: Default::default(),
    });
    let bytes = postcard::to_allocvec(&request).unwrap();
    let data: String = bytes.iter().map(|byte| format!("\\{byte:02x}")).collect();
    let program = wat::parse_str(format!(
        r#"(module
            (import "ship_v30" "world_query" (func $query (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{data}")
            (func (export "ship_api_version") (result i32) i32.const {version})
            (func (export "ship_tick")
                i32.const 0 i32.const {length} i32.const 256 i32.const 128 call $query
                i32.const 0 i32.lt_s if unreachable end))"#,
        version = abi::VERSION,
        length = bytes.len(),
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
    let source = Arc::new(JobSource {
        calls: AtomicUsize::new(0),
    });
    let first = computer
        .run_slice(Input::default(), Some(source.clone()), 8192, FUEL_PER_TICK)
        .unwrap();
    assert!(!first.callback_completed);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert!(computer.minimum_to_progress() > routing::REQUEST_GAS);
    assert!(computer.last_gas_used <= 8192);

    let second = computer
        .run_slice(Input::default(), Some(source.clone()), 9000, FUEL_PER_TICK)
        .unwrap();
    assert!(second.callback_completed);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(computer.last_gas_used >= routing::REQUEST_GAS);
    assert!(computer.last_gas_used <= 9000);
    assert!(computer.fault.is_none());
}
