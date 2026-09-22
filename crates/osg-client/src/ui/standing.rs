use osg_model::{
    IffIdentity,
    ownership::{Principal, Standing},
};
use osg_ui::{desktop::THREAT, egui};

pub(super) fn advertised_principal(iff: Option<&IffIdentity>) -> Option<Principal> {
    iff.filter(|iff| iff.enabled)
        .map(|iff| Principal::Player(iff.owner))
}

pub(super) fn color(standing: Option<Standing>) -> egui::Color32 {
    match standing {
        Some(Standing::Friendly) => egui::Color32::from_rgb(115, 225, 145),
        Some(Standing::Neutral) => egui::Color32::WHITE,
        Some(Standing::Hostile) => THREAT,
        None => egui::Color32::WHITE,
    }
}

pub(super) fn label(standing: Option<Standing>) -> &'static str {
    match standing {
        Some(Standing::Friendly) => "Friendly",
        Some(Standing::Neutral) => "Neutral",
        Some(Standing::Hostile) => "Hostile",
        None => "Unknown identity",
    }
}

pub(super) fn symbol(standing: Option<Standing>) -> &'static str {
    match standing {
        Some(Standing::Friendly) => "+",
        Some(Standing::Neutral) => "=",
        Some(Standing::Hostile) => "−",
        None => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::{
        Id,
        ownership::{OwnershipDirectory, Principal},
    };
    use std::collections::BTreeSet;

    #[test]
    fn colors_follow_advertised_identity_and_keep_unknown_distinct() {
        let observer = Id([1; 16]);
        let advertised = Id([2; 16]);
        let mut directory = OwnershipDirectory::default();
        directory.standings.insert(
            (Principal::Player(observer), Principal::Player(advertised)),
            Standing::Hostile,
        );
        let iff = IffIdentity {
            owner: advertised,
            faction: None,
            labels: BTreeSet::new(),
            enabled: true,
        };
        assert_eq!(
            color(directory.contact_standing(observer, Some(&iff))),
            THREAT
        );
        assert_eq!(directory.contact_standing(observer, None), None);
        assert_eq!(color(None), color(Some(Standing::Neutral)));
    }
}
