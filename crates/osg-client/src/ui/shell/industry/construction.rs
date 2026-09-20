use super::*;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
pub(in crate::ui::shell) struct Request {
    pub facility: Id,
    pub facility_name: String,
    pub owner: ownership::Principal,
    pub name: String,
    pub bytes: Arc<[u8]>,
}

type Session = (Id, u64);
type UploadResult = Result<[u8; 32], String>;

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Queued,
    Uploading(oneshot::Receiver<UploadResult>),
    Submitted(Id),
    UploadFailed(String),
    BuildFailed(String),
    Accepted,
}

#[derive(Default)]
pub(in crate::ui::shell) struct State {
    request: Option<Request>,
    session: Option<Session>,
    status: Status,
}

impl State {
    pub fn busy(&self) -> bool {
        matches!(
            self.status,
            Status::Queued | Status::Uploading(_) | Status::Submitted(_)
        )
    }

    pub fn queue(&mut self, request: Request, session: Session) {
        if self.busy() {
            return;
        }

        self.request = Some(request);
        self.session = Some(session);
        self.status = Status::Queued;
    }

    fn start(&mut self, request: Request, session: Session, client: crate::AssetClient) {
        let bytes = request.bytes.clone();
        let (send, receive) = oneshot::channel();
        self.begin(request, session, receive);
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let result = client
                    .upload_blueprint(bytes)
                    .await
                    .map_err(|error| error.to_string());
                let _ = send.send(result);
            })
            .detach();
    }

    fn begin(
        &mut self,
        request: Request,
        session: Session,
        receive: oneshot::Receiver<UploadResult>,
    ) {
        self.request = Some(request);
        self.session = Some(session);
        self.status = Status::Uploading(receive);
    }

    fn update(
        &mut self,
        session: Option<Session>,
        results: &[CommandResult],
        outgoing: &mut Outgoing,
    ) -> Option<Id> {
        if self.session != session {
            *self = Self::default();
            return None;
        }

        match &mut self.status {
            Status::Uploading(receive) => {
                let result = match receive.try_recv() {
                    Ok(result) => result,
                    Err(oneshot::error::TryRecvError::Empty) => return None,
                    Err(oneshot::error::TryRecvError::Closed) => {
                        Err("Blueprint upload was interrupted".into())
                    }
                };
                match result {
                    Ok(blueprint_hash) => {
                        let request = self.request.as_ref().unwrap();
                        let command = outgoing.push(Action::Industry(IndustryCommand::BuildShip {
                            facility: request.facility,
                            owner: request.owner,
                            blueprint_hash,
                        }));
                        self.status = Status::Submitted(command);
                        return Some(command);
                    }
                    Err(error) => self.status = Status::UploadFailed(error),
                }
            }
            Status::Submitted(command) => {
                if let Some(result) = results.iter().find(|result| result.id == *command) {
                    self.status = match &result.error {
                        Some(error) => Status::BuildFailed(error.clone()),
                        None => Status::Accepted,
                    };
                }
            }
            _ => {}
        }
        None
    }

    pub fn draw(&self, ui: &mut egui::Ui, connected: bool, intents: &mut Vec<Intent>) {
        let Some(request) = &self.request else {
            return;
        };
        ui.small(format!("{} · {}", request.name, request.facility_name));
        match &self.status {
            Status::Queued | Status::Uploading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Uploading blueprint and installed firmware…");
                });
            }
            Status::Submitted(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Blueprint ready · awaiting shipyard acceptance…");
                });
            }
            Status::UploadFailed(error) => {
                ui.colored_label(THREAT, format!("Upload failed: {error}"));
                if ui
                    .add_enabled(connected, egui::Button::new("Retry upload"))
                    .clicked()
                {
                    intents.push(Intent::BuildShip(request.clone()));
                }
            }
            Status::BuildFailed(error) => {
                ui.colored_label(THREAT, format!("Build rejected: {error}"));
            }
            Status::Accepted => {
                ui.colored_label(
                    ACCENT,
                    "Shipyard accepted the build. Follow progress in Jobs.",
                );
            }
            Status::Idle => {}
        }
        ui.separator();
    }
}

pub(in crate::ui::shell) fn update(
    mut shell: ResMut<Shell>,
    session: Res<SessionInfo>,
    mut outgoing: ResMut<Outgoing>,
    client: Res<crate::ui::BlueprintAssets>,
) {
    let key = session
        .world
        .filter(|_| session.status.is_empty())
        .map(|world| (world, session.generation));
    if let Some(command) = shell
        .industry
        .construction
        .update(key, &session.results, &mut outgoing)
    {
        shell.feedback = Some(Feedback {
            pending: vec![command],
            label: "Build ship".into(),
            last_tick: 0,
            error: None,
        });
    }

    let construction = &mut shell.industry.construction;
    if matches!(construction.status, Status::Queued) {
        construction.start(
            construction.request.clone().unwrap(),
            key.unwrap(),
            client.0.clone(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            facility: Id([2; 16]),
            facility_name: "Original shipyard".into(),
            owner: ownership::Principal::Player(Id([3; 16])),
            name: "Custom firmware cutter".into(),
            bytes: Arc::from(vec![7; 100_000]),
        }
    }

    #[test]
    fn blueprint_upload_ack_submits_frozen_context_exactly_once_and_waits_for_build_result() {
        let key = Some((Id([1; 16]), 4));
        let (send, receive) = oneshot::channel();
        let mut state = State::default();
        let mut outgoing = Outgoing::default();
        state.begin(request(), key.unwrap(), receive);
        assert!(state.busy());
        assert!(state.update(key, &[], &mut outgoing).is_none());
        assert!(outgoing.pending().is_empty());

        let mut other = request();
        other.facility = Id([8; 16]);
        other.owner = ownership::Principal::Player(Id([8; 16]));
        state.queue(other, key.unwrap());

        send.send(Ok([9; 32])).unwrap();
        let command = state.update(key, &[], &mut outgoing).unwrap();
        assert!(
            matches!(&outgoing.pending()[..], [(_, Action::Industry(IndustryCommand::BuildShip {
            facility, owner, blueprint_hash,
        }))] if *facility == Id([2; 16])
            && *owner == ownership::Principal::Player(Id([3; 16]))
            && *blueprint_hash == [9; 32])
        );
        assert!(state.busy());
        assert!(state.update(key, &[], &mut outgoing).is_none());
        assert_eq!(outgoing.pending().len(), 1);

        let unrelated = CommandResult {
            id: Id([8; 16]),
            effective_tick: 8,
            error: None,
            reply: None,
        };
        state.update(key, &[unrelated], &mut outgoing);
        assert!(state.busy());

        let result = CommandResult {
            id: command,
            effective_tick: 9,
            error: Some("No free shipyard lane".into()),
            reply: None,
        };
        state.update(key, &[result], &mut outgoing);
        assert!(!state.busy());
        assert!(matches!(state.status, Status::BuildFailed(_)));
    }

    #[test]
    fn blueprint_upload_failure_and_session_reset_never_submit_a_build() {
        let key = Some((Id([1; 16]), 4));
        let mut state = State::default();
        let mut outgoing = Outgoing::default();
        let (send, receive) = oneshot::channel();
        state.begin(request(), key.unwrap(), receive);
        send.send(Err("Upload quota exceeded".into())).unwrap();
        state.update(key, &[], &mut outgoing);
        assert!(matches!(state.status, Status::UploadFailed(_)));
        assert!(outgoing.pending().is_empty());

        let (send, receive) = oneshot::channel();
        state.begin(request(), key.unwrap(), receive);
        send.send(Ok([9; 32])).unwrap();
        state.update(Some((Id([1; 16]), 5)), &[], &mut outgoing);
        assert!(!state.busy());
        assert!(state.request.is_none());
        assert!(outgoing.pending().is_empty());
    }
}
