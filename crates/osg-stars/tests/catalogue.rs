use osg_stars::{GalacticPosition as P, Star, StarCatalogue, StarId, VisibilityQuery};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
fn temporary() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "osg-stars-{}-{}.stars",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}
fn star(value: u64, position: P, luminosity: f64) -> Star {
    Star {
        id: StarId::gaia(value),
        position,
        luminosity,
        colour: [1.; 3],
    }
}
#[test]
fn queries_match_brute_force_at_translated_origins_and_limits() {
    let mut seed = 9u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let stars: Vec<_> = (0..2000)
        .map(|i| {
            star(
                i,
                P::new(
                    (next() % 20_000_000_001) as i128 - 10_000_000_000,
                    (next() % 20_000_000_001) as i128 - 10_000_000_000,
                    (next() % 20_000_000_001) as i128 - 10_000_000_000,
                ),
                2f64.powi((i % 40) as i32),
            )
        })
        .collect();
    for anchor in [P::ZERO, P::new(1 << 90, -(1 << 90), 1 << 85)] {
        let stars: Vec<_> = stars
            .iter()
            .map(|s| Star {
                position: s.position + anchor,
                ..*s
            })
            .collect();
        let catalogue = StarCatalogue::from_stars(stars.clone()).unwrap();
        for offset in [
            P::ZERO,
            P::new(-100_000_000, 200_000_000, 0),
            P::new(90_000_000_000, 0, 0),
        ] {
            let origin = anchor + offset;
            let distance = |s: &Star| s.position.relative_to(origin).length_squared();
            let expected = stars
                .iter()
                .enumerate()
                .min_by(|a, b| distance(a.1).total_cmp(&distance(b.1)))
                .unwrap()
                .0;
            assert_eq!(catalogue.nearest(origin).unwrap().unwrap().0, expected);
            for radius in [0., 100., 5000., 100_000.] {
                let mut actual = catalogue.within_radius(origin, radius).unwrap();
                actual.sort_unstable();
                let expected: Vec<_> = stars
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| distance(s) <= radius * radius)
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(actual, expected);
            }
            for cutoff in [0., 1e-6, 0.01, 1., 1e10] {
                let excluded = [StarId::gaia(42), StarId::gaia(9)];
                let mut expected: Vec<_> = stars
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        !excluded.contains(&s.id)
                            && distance(s) > 0.
                            && s.luminosity / distance(s) >= cutoff
                    })
                    .map(|(i, _)| i)
                    .collect();
                expected.sort_by(|&a, &b| {
                    (stars[b].luminosity / distance(&stars[b]))
                        .total_cmp(&(stars[a].luminosity / distance(&stars[a])))
                        .then(a.cmp(&b))
                });
                for cap in [0, 7, usize::MAX] {
                    let result = catalogue
                        .visible(
                            origin,
                            VisibilityQuery {
                                min_brightness: cutoff,
                                max_stars: cap,
                                excluded: &excluded,
                            },
                        )
                        .unwrap();
                    assert_eq!(result.matched, expected.len());
                    assert_eq!(result.indices, expected[..expected.len().min(cap)]);
                }
            }
        }
    }
}
#[test]
fn luminosity_buckets_skip_faint_populations_and_handle_duplicate_positions() {
    let mut stars: Vec<_> = (0..5000)
        .map(|i| star(i, P::new(100_000_000_000, 0, 0), 1.))
        .collect();
    stars.push(star(9999, P::new(100_000_000_000, 0, 0), 1e12));
    let catalogue = StarCatalogue::from_stars(stars).unwrap();
    let visible = catalogue
        .visible(
            P::ZERO,
            VisibilityQuery {
                min_brightness: 1.,
                max_stars: 10,
                excluded: &[],
            },
        )
        .unwrap();
    assert_eq!(visible.indices, [5000]);
    assert_eq!(visible.candidates, 1);
}
#[test]
fn floating_index_does_not_lose_close_pairs_or_boundary_matches() {
    let far = P::new(1 << 90, 0, 0);
    let catalogue = StarCatalogue::from_stars(vec![
        star(0, P::ZERO, 1.),
        star(1, far, 1.),
        star(2, far + P::new(1, 0, 0), 1.),
    ])
    .unwrap();
    assert_eq!(
        catalogue.nearest(far + P::new(1, 0, 0)).unwrap().unwrap(),
        (2, 0.)
    );
    let mut ids = catalogue.within_radius(far, 1e-6).unwrap();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2]);
    let result = catalogue
        .visible(
            far,
            VisibilityQuery {
                min_brightness: 1e12,
                max_stars: 10,
                excluded: &[],
            },
        )
        .unwrap();
    assert_eq!(result.indices, [2]);
}
#[test]
fn file_roundtrip_and_rejection_of_invalid_formats_and_records() {
    let path = temporary();
    let catalogue =
        StarCatalogue::from_stars(vec![star(123, P::new(1 << 90, -(1 << 90), 1), 1e20)]).unwrap();
    catalogue.save(&path, StarId::GAIA_DR3).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_eq!(original.len(), 116);
    let restored = StarCatalogue::load(&path).unwrap();
    assert_eq!(
        restored.star(StarId::gaia(123)).unwrap().position,
        catalogue.stars()[0].position
    );
    for byte in [0, 8, 12, 16, 24, 28] {
        let mut invalid = original.clone();
        invalid[byte] ^= 2;
        std::fs::write(&path, &invalid).unwrap();
        assert!(StarCatalogue::load(&path).is_err());
    }
    std::fs::write(&path, &original[..original.len() - 1]).unwrap();
    assert!(StarCatalogue::load(&path).is_err());
    let mut invalid = original.clone();
    invalid[96..104].copy_from_slice(&f64::NAN.to_le_bytes());
    std::fs::write(&path, invalid).unwrap();
    assert!(StarCatalogue::load(&path).is_err());
    std::fs::remove_file(path).unwrap();
    let s = star(1, P::ZERO, 1.);
    assert!(StarCatalogue::from_stars(vec![s, s]).is_err());
    let other = Star {
        id: StarId {
            namespace: 99,
            value: 1,
        },
        ..s
    };
    assert_eq!(StarCatalogue::from_stars(vec![s, other]).unwrap().len(), 2);
    let empty = StarCatalogue::from_stars(vec![]).unwrap();
    assert!(empty.nearest(P::ZERO).unwrap().is_none());
    assert!(
        empty
            .visible(P::ZERO, VisibilityQuery::magnitude(6.))
            .unwrap()
            .indices
            .is_empty()
    );
}
#[test]
fn python_converter_fixture_matches_portable_loader() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = temporary();
    let output = std::process::Command::new("python3")
        .arg(root.join("tools/import_gaia.py"))
        .arg(&path)
        .arg(root.join("tests/fixtures/gaia_sample.csv"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalogue = StarCatalogue::load(&path).unwrap();
    assert_eq!(catalogue.len(), 4);
    assert_eq!(
        catalogue
            .stars()
            .iter()
            .map(|s| s.id.value)
            .collect::<Vec<_>>(),
        [1, 2, 3, 6]
    );
    assert!(
        (catalogue.stars()[0].position.relative_to(P::ZERO).length() / osg_stars::PARSEC - 10.)
            .abs()
            < 1e-12
    );
    for s in catalogue.stars() {
        assert!(
            (s.colour[0] * 0.2126 + s.colour[1] * 0.7152 + s.colour[2] * 0.0722 - 1.).abs() < 1e-5
        );
    }
    let first = std::fs::read(&path).unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("tools/import_gaia.py"))
        .arg(&path)
        .arg(root.join("tests/fixtures/gaia_sample.csv"))
        .arg(root.join("tests/fixtures/gaia_sample.csv"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), first);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_extension("stars.json")).unwrap();
}

#[test]
fn embedded_million_star_catalogue_is_queryable_without_a_runtime_file() {
    let catalogue = StarCatalogue::embedded().unwrap();
    assert_eq!(catalogue.len(), 1_000_000);
    let visible = catalogue
        .visible(P::ZERO, VisibilityQuery::magnitude(6.0))
        .unwrap();
    assert!((6_000..7_000).contains(&visible.matched));
    assert_eq!(visible.indices.len(), visible.matched);
    assert!(visible.candidates < 20_000);
    assert!(StarCatalogue::from_bytes(b"truncated").is_err());
}
