use osg_model::Tag;
use osg_ui::icons::Icon;
use std::collections::BTreeSet;

pub(super) fn kind(tags: &BTreeSet<Tag>) -> &str {
    if tags
        .iter()
        .any(|tag| matches!(tag, Tag::Kind(kind) if kind == "missile"))
    {
        return "Missile";
    }

    tags.iter()
        .find_map(|tag| match tag {
            Tag::Kind(kind) => Some(kind.as_str()),
            _ => None,
        })
        .unwrap_or("Contact")
}

pub(super) fn icon(kind: &str) -> Icon {
    if kind.eq_ignore_ascii_case("missile") {
        Icon::Missile
    } else {
        Icon::Ship
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missile_classification_takes_priority_over_generic_ship_tags() {
        let tags = BTreeSet::from([
            Tag::Kind("ship".into()),
            Tag::Kind("missile".into()),
            Tag::Advertised("Interceptor".into()),
        ]);
        assert_eq!(kind(&tags), "Missile");
        assert_eq!(icon(kind(&tags)).glyph(), Icon::Missile.glyph());
        assert_ne!(Icon::Missile.glyph(), Icon::Ship.glyph());
        assert_eq!(kind(&BTreeSet::new()), "Contact");
    }
}
