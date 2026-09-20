use osg_model::{
    Tag,
    ownership::{Principal, Standing},
};
use osg_ui::{desktop::THREAT, egui};
use std::collections::BTreeSet;

pub(super) fn advertised_principal(tags: &BTreeSet<Tag>) -> Option<Principal> {
    tags.iter()
        .find_map(|tag| match tag {
            Tag::IffOwner(id) => Some(Principal::Player(*id)),
            _ => None,
        })
        .or_else(|| {
            tags.iter().find_map(|tag| match tag {
                Tag::IffFaction(id) => Some(Principal::Organization(*id)),
                _ => None,
            })
        })
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
        Id, Tag,
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
        let tags = BTreeSet::from([Tag::IffOwner(advertised)]);
        assert_eq!(color(directory.track_standing(observer, &tags)), THREAT);
        assert_eq!(directory.track_standing(observer, &BTreeSet::new()), None);
        assert_eq!(color(None), color(Some(Standing::Neutral)));
    }
}
