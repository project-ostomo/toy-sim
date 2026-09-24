use super::Outgoing;
use osg_model::{Action, Id, chat::*};
use std::collections::VecDeque;

pub(super) const RETAINED_MESSAGES: usize = 2_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChatFocus {
    pub view: u64,
    pub view_revision: u64,
    pub ship: Id,
}

#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct ChatState {
    pub messages: VecDeque<ChatMessage>,
    pub generation: u64,
    pub missed: u64,
    pub unavailable: bool,
    pub interrupted: bool,
    focus: Option<ChatFocus>,
    subscription: Option<ChatFocus>,
    revision: u64,
    received: bool,
}

impl ChatState {
    pub fn loading(&self) -> bool {
        self.subscription.is_some() && !self.received
    }

    pub fn subscribe(&mut self, wanted: Option<ChatFocus>, outgoing: &mut Outgoing) {
        if self.subscription == wanted {
            return;
        }

        self.received = false;
        self.subscription = wanted;
        if let Some(focus) = wanted {
            if self.focus != Some(focus) {
                self.messages.clear();
                self.generation = self
                    .generation
                    .checked_add(1)
                    .expect("chat generation exhausted");
                self.missed = 0;
                self.unavailable = false;
                self.interrupted = false;
                self.focus = Some(focus);
            }
            self.revision = self
                .revision
                .checked_add(1)
                .expect("chat revision exhausted");
            outgoing.push(Action::ChatSubscribe(ChatSubscription {
                revision: self.revision,
                view: focus.view,
            }));
        } else {
            self.interrupted |= !self.messages.is_empty();
            outgoing.push(Action::ChatUnsubscribe);
        }
    }

    pub(super) fn apply(&mut self, update: ChatUpdate) {
        let Some(focus) = self.subscription else {
            return;
        };
        if update.subscription_revision != self.revision
            || update.view != focus.view
            || update.view_revision != focus.view_revision
        {
            return;
        }

        self.received = true;
        self.unavailable = update.unavailable;
        self.missed = self.missed.saturating_add(update.page.missed);
        for message in update.page.messages {
            if self
                .messages
                .back()
                .is_some_and(|last| message.sequence <= last.sequence)
            {
                continue;
            }
            self.messages.push_back(message);
        }
        let excess = self.messages.len().saturating_sub(RETAINED_MESSAGES);
        self.messages.drain(..excess);
    }

    pub fn can_send(&self) -> bool {
        self.subscription.is_some() && self.received && !self.unavailable
    }

    pub fn transmit(&self, text: String, outgoing: &mut Outgoing) -> Option<Id> {
        self.can_send().then(|| {
            outgoing.push(Action::ChatSend {
                subscription_revision: self.revision,
                text,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(revision: u64, view_revision: u64, start: u64, count: u64) -> ChatUpdate {
        ChatUpdate {
            subscription_revision: revision,
            view: 1,
            view_revision,
            unavailable: false,
            page: ChatPage {
                messages: (start..start + count)
                    .map(|sequence| ChatMessage {
                        id: Id(sequence.to_le_bytes().repeat(2).try_into().unwrap()),
                        sequence,
                        tick: sequence,
                        calendar_unix_ms: 0,
                        sender_name: "Courier".into(),
                        advertised_owner: None,
                        advertised_organization: None,
                        text: sequence.to_string(),
                    })
                    .collect(),
                next_sequence: start + count - 1,
                missed: 0,
            },
        }
    }

    #[test]
    fn subscriptions_reject_stale_views_and_keep_same_channel_history_on_reopen() {
        let mut chat = ChatState::default();
        let mut outgoing = Outgoing::default();
        let focus = ChatFocus {
            view: 1,
            view_revision: 7,
            ship: Id([1; 16]),
        };
        chat.subscribe(Some(focus), &mut outgoing);
        chat.subscribe(Some(focus), &mut outgoing);
        assert_eq!(outgoing.pending().len(), 1);
        assert!(!chat.can_send());

        chat.apply(update(1, 7, 1, 2));
        chat.apply(update(1, 7, 2, 1));
        assert_eq!(chat.messages.len(), 2);
        assert!(chat.can_send());
        chat.subscribe(None, &mut outgoing);
        assert!(!chat.can_send());
        assert!(chat.interrupted);
        chat.subscribe(Some(focus), &mut outgoing);
        chat.apply(update(1, 7, 3, 1));
        assert!(!chat.can_send());
        chat.apply(update(2, 7, 3, 1));
        assert_eq!(chat.messages.len(), 3);

        chat.subscribe(
            Some(ChatFocus {
                view_revision: 8,
                ship: Id([2; 16]),
                ..focus
            }),
            &mut outgoing,
        );
        assert!(chat.messages.is_empty());
        chat.apply(update(2, 7, 4, 1));
        chat.apply(update(3, 7, 4, 1));
        assert!(chat.messages.is_empty());
        chat.apply(update(3, 8, 1, 1));
        assert_eq!(chat.messages[0].sequence, 1);
        assert!(!chat.interrupted);
    }

    #[test]
    fn retained_history_is_bounded_without_limiting_incoming_delivery() {
        let mut chat = ChatState::default();
        chat.subscribe(
            Some(ChatFocus {
                view: 1,
                view_revision: 7,
                ship: Id([1; 16]),
            }),
            &mut Outgoing::default(),
        );
        for start in (1..=4_001).step_by(16) {
            chat.apply(update(1, 7, start, 16));
        }
        assert_eq!(chat.messages.len(), RETAINED_MESSAGES);
        assert_eq!(chat.messages.back().unwrap().sequence, 4_016);
        assert_eq!(chat.messages.front().unwrap().sequence, 2_017);
        assert_eq!(chat.missed, 0);
    }
}
