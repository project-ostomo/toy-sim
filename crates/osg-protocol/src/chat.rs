use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use osg_model::chat::*;

pub fn validate_update(update: &ChatUpdate) -> Result<()> {
    ensure!(
        update.subscription_revision > 0,
        "invalid chat subscription revision"
    );
    ensure!(
        !update.unavailable || update.page == ChatPage::default(),
        "unavailable chat exposes history"
    );
    validate_page(&update.page)
}

fn validate_page(page: &ChatPage) -> Result<()> {
    ensure!(
        page.messages.len() <= MAX_PAGE_MESSAGES,
        "too many chat messages"
    );
    let mut sequence = None;
    let mut ids = BTreeSet::new();
    for message in &page.messages {
        ensure!(
            message.sequence > 0
                && sequence
                    .is_none_or(|previous: u64| previous.checked_add(1) == Some(message.sequence)),
            "invalid chat message order"
        );
        ensure!(ids.insert(message.id), "duplicate chat message ID");
        ensure!(valid_text(&message.text), "invalid chat text");
        ensure!(
            !message.sender_name.trim().is_empty()
                && message.sender_name.len() <= MAX_SENDER_BYTES
                && !message.sender_name.chars().any(char::is_control),
            "invalid chat sender"
        );
        sequence = Some(message.sequence);
    }
    ensure!(
        sequence.is_none_or(|sequence| sequence == page.next_sequence),
        "invalid chat cursor"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::Id;

    fn page() -> ChatPage {
        ChatPage {
            messages: vec![ChatMessage {
                id: Id::new(),
                sequence: 2,
                tick: 4,
                calendar_unix_ms: 0,
                sender_name: "Unidentified transmission".into(),
                advertised_owner: None,
                advertised_organization: None,
                text: "Hello, local.".into(),
            }],
            next_sequence: 2,
            missed: 1,
        }
    }

    #[test]
    fn chat_pages_validate_cursor_identity_and_unicode_byte_limits() {
        let mut page = page();
        assert!(validate_page(&page).is_ok());
        page.messages[0].text = "界".repeat(MAX_MESSAGE_BYTES / 3);
        assert!(validate_page(&page).is_ok());
        page.messages[0].text.push('界');
        assert!(validate_page(&page).is_err());
        page.messages[0].text = "valid".into();
        page.next_sequence += 1;
        assert!(validate_page(&page).is_err());
        page.next_sequence -= 1;
        let mut duplicate = page.messages[0].clone();
        duplicate.sequence += 1;
        page.next_sequence += 1;
        page.messages.push(duplicate);
        assert!(validate_page(&page).is_err());
    }
}
