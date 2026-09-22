use super::*;
use std::collections::VecDeque;

fn message(sequence: u64, text: String) -> ChatMessage {
    ChatMessage {
        id: Id(sequence.to_le_bytes().repeat(2).try_into().unwrap()),
        sequence,
        tick: sequence,
        calendar_unix_ms: osg_model::calendar::from_real_unix_ms(1_789_689_600_000),
        sender_name: format!("Courier {sequence}"),
        advertised_owner: Some(Id([2; 16])),
        advertised_organization: Some(Id([3; 16])),
        text,
    }
}

struct Fixture {
    society: ownership::SocietySnapshot,
    navigation: NavigationCatalogue,
    ship: ShipTelemetry,
    industry: industry_model::IndustrySnapshot,
}

impl Fixture {
    fn new() -> Self {
        Self {
            society: ownership::SocietySnapshot {
                account: Id([1; 16]),
                ..Default::default()
            },
            navigation: Default::default(),
            ship: crate::ui::tests::ship(Id([4; 16])).0,
            industry: Default::default(),
        }
    }

    fn model(&self) -> FrameModel<'_> {
        FrameModel {
            industry: &self.industry,
            industry_ready: true,
            society: &self.society,
            navigation: &self.navigation,
            inhabited: Default::default(),
            navigation_status: &NavigationStatus::Ready,
            navigation_hash: None,
            ships: vec![&self.ship],
            rows: Vec::new(),
            ship: Some(&self.ship),
            details: None,
            system: "Helion".into(),
            vicinity: String::new(),
            connected: true,
            status: "",
            time_ns: 0,
            calendar_unix_ms: None,
            diagnostics: Default::default(),
            orbits: false,
        }
    }
}

fn render(
    ctx: &egui::Context,
    frame: usize,
    events: Vec<egui::Event>,
    draw: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Pos2)> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(620.0, 460.0),
            )),
            time: Some(frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        draw,
    );
    output.textures_delta.clear();
    let mut labels = Vec::new();
    for shape in output.shapes {
        if let egui::Shape::Text(text) = shape.shape {
            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
            if shape.clip_rect.intersects(rect) {
                labels.push((text.galley.job.text.clone(), rect.center()));
            }
        }
    }
    labels
}

#[test]
fn long_unicode_scrollback_virtualizes_and_keeps_reader_position_during_new_messages() {
    let ctx = egui::Context::default();
    osg_ui::theme::install(&ctx);
    let fixture = Fixture::new();
    let mut chat = ChatState::default();
    chat.messages = (1..=2_000)
        .map(|sequence| {
            message(
                sequence,
                format!(
                    "Message {sequence}: {}\nNext paragraph",
                    "星際 freight corridor ".repeat(8)
                ),
            )
        })
        .collect::<VecDeque<_>>();
    let mut state = State::default();
    let mut intents = Vec::new();
    let mut labels = Vec::new();
    let started = std::time::Instant::now();
    for frame in 0..5 {
        labels = render(&ctx, frame, Vec::new(), |ui| {
            draw(ui, &mut state, &fixture.model(), &chat, &mut intents)
        });
    }
    eprintln!(
        "chat first layout and5frames: {:?}, {} cached visual rows",
        started.elapsed(),
        state.layout.lines.len()
    );
    assert!(state.layout.lines.len() > 6_000);
    assert!(labels.len() < 50, "offscreen history was painted");
    assert!(labels.iter().any(|(text, _)| text.contains("Courier 2000")));
    assert!(state.at_bottom);

    for frame in 5..10 {
        render(
            &ctx,
            frame,
            vec![
                egui::Event::PointerMoved(egui::pos2(200.0, 150.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0.0, 180.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            |ui| draw(ui, &mut state, &fixture.model(), &chat, &mut intents),
        );
    }
    for frame in 10..35 {
        labels = render(&ctx, frame, Vec::new(), |ui| {
            draw(ui, &mut state, &fixture.model(), &chat, &mut intents)
        });
    }
    assert!(!state.at_bottom);
    let old_offset = state.scroll_offset;
    let old_heading = labels
        .iter()
        .find(|(text, _)| text.contains("Courier "))
        .unwrap()
        .0
        .clone();

    chat.messages
        .push_back(message(2_001, "New message".into()));
    labels = render(&ctx, 35, Vec::new(), |ui| {
        draw(ui, &mut state, &fixture.model(), &chat, &mut intents)
    });
    assert!((state.scroll_offset - old_offset).abs() < 1.0);
    assert!(labels.iter().any(|(text, _)| *text == old_heading));

    chat.messages.pop_front();
    labels = render(&ctx, 36, Vec::new(), |ui| {
        draw(ui, &mut state, &fixture.model(), &chat, &mut intents)
    });
    assert!(labels.iter().any(|(text, _)| *text == old_heading));
    assert!(!state.at_bottom);
}

#[test]
fn sender_color_and_organization_follow_only_advertised_identity() {
    let mut society = ownership::SocietySnapshot {
        account: Id([1; 16]),
        ..Default::default()
    };
    let mut message = message(1, "Hello".into());
    let advertised = ownership::Principal::Organization(Id([3; 16]));
    society.directory.organizations.insert(
        Id([3; 16]),
        ownership::Organization {
            id: Id([3; 16]),
            name: "Declared Cooperative".into(),
            sovereignty: Id([8; 16]),
            open_membership: true,
            officers: Default::default(),
        },
    );
    society.directory.standings.insert(
        (ownership::Principal::Player(society.account), advertised),
        ownership::Standing::Hostile,
    );
    assert_eq!(sender_color(&message, &society), THREAT);
    assert!(heading(&message, &society.directory).contains("2426-09-18 00:00:00"));
    assert!(heading(&message, &society.directory).contains("Declared Cooperative"));

    message.advertised_owner = None;
    message.advertised_organization = None;
    assert_eq!(
        sender_color(&message, &society),
        super::super::super::standing::color(None)
    );
    assert!(!heading(&message, &society.directory).contains("Cooperative"));
}
