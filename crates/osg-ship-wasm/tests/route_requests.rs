use osg_model::{ProgramQuery, ProgramReply, routing, travel};
use osg_ship_api::{abi::Record, world};
use osg_ship_wasm::{ControllerRuntime, FUEL_PER_TICK, Input, ScanSource, SensorContact};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct JobSource {
    calls: AtomicUsize,
}

impl ScanSource for JobSource {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(
        &self,
        query: ProgramQuery,
        _: bool,
        _: osg_model::wasm_world::ReplyCapacity,
    ) -> anyhow::Result<ProgramReply> {
        let ProgramQuery::RouteRequest(request) = query else {
            anyhow::bail!("unexpected query");
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.id, 42);
        assert_eq!(request.orders, vec![travel::Order::WaitUntil(10)]);
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

fn program(header: u32) -> Vec<u8> {
    let request = world::RouteRequest {
        id: 42,
        preferences: world::Preferences {
            fuel_fraction: 0.5,
            max_loss_ppm: 100.0,
            allow_slipdrive: 1,
        },
    };
    let order = world::Order::from(&travel::Order::WaitUntil(10));
    let mut bytes = request.bytes().to_vec();
    bytes.extend_from_slice(order.bytes());
    let data: String = bytes.iter().map(|byte| format!("\\{byte:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
            (import "ship" "route_request" (func $query (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{data}")
            (func (export "game_version") (result i32) i32.const {version})
            (func (export "ship_tick")
                i32.const 0 i32.const {order_offset} i32.const 1 i32.const {header}
                i32.const 4096 i32.const 0 i32.const 8192 i32.const 0 call $query
                i32.const 0 i32.lt_s if unreachable end))"#,
        version = osg_ship_api::GAME_VERSION as u32,
        order_offset = std::mem::size_of::<world::RouteRequest>(),
    ))
    .unwrap()
}

#[test]
fn route_request_prepays_bounded_admission_and_resumes_exactly_once() {
    let program = program(1024);
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

#[test]
fn invalid_route_output_range_never_submits_a_job() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(&program(65530)).unwrap();
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
    let result = computer.run_slice(
        Input::default(),
        Some(source.clone()),
        FUEL_PER_TICK,
        FUEL_PER_TICK,
    );
    assert!(result.is_err() || computer.fault.is_some());
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
}
