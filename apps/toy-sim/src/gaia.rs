//! Bevy adapter: index the embedded star catalogue and query it off-thread.
use crate::{
    camera::MainCamera,
    precision::{GalacticPosition, PreciseTransform},
    starfield::SkySettings,
};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::{sync::Arc, time::Instant};
pub use toy_sim_stars::Star;
use toy_sim_stars::{StarCatalogue, StarId, VisibilityQuery, min_brightness};
// Rendering workload limit, independent of catalogue size and universe data.
const MAX_SELECTED_STARS: usize = 150_000;
fn select(
    reader: &StarCatalogue,
    origin: GalacticPosition,
    cutoff: f64,
    limit: usize,
    exclude: &[u64],
) -> anyhow::Result<Selection> {
    let start = Instant::now();
    let excluded: Vec<_> = exclude.iter().copied().map(StarId::gaia).collect();
    let result = reader.visible(
        origin,
        VisibilityQuery {
            min_brightness: cutoff,
            max_stars: limit,
            excluded: &excluded,
        },
    )?;
    let nearest = reader.nearest(origin)?.map_or(f64::INFINITY, |(_, d)| d);
    Ok(Selection {
        stars: result.indices.iter().map(|&i| reader.stars()[i]).collect(),
        origin,
        budget: (nearest * 0.01).max(1.),
        seconds: start.elapsed().as_secs_f64(),
        matched: result.matched,
        cutoff,
        candidates: result.candidates,
        buckets_searched: result.buckets_searched,
        total_stars: reader.len(),
    })
}
struct Selection {
    stars: Vec<Star>,
    origin: GalacticPosition,
    budget: f64,
    seconds: f64,
    matched: usize,
    cutoff: f64,
    candidates: usize,
    total_stars: usize,
    buckets_searched: usize,
}
#[derive(Resource)]
pub struct GaiaCatalogue {
    reader: Option<Arc<StarCatalogue>>,
    task: Option<Task<anyhow::Result<Selection>>>,
    opening: Option<Task<anyhow::Result<Arc<StarCatalogue>>>>,
    current: Option<Selection>,
    failed: bool,
    pub state: String,
    pub revision: u64,
    pub selected: usize,
    pub matched: usize,
    pub seconds: f64,
    pub candidates: usize,
    pub buckets_searched: usize,
    pub total_stars: usize,
}
impl Default for GaiaCatalogue {
    fn default() -> Self {
        Self {
            reader: None,
            task: None,
            opening: None,
            current: None,
            failed: false,
            state: "Indexing embedded catalogue".into(),
            revision: 0,
            selected: 0,
            matched: 0,
            seconds: 0.,
            candidates: 0,
            buckets_searched: 0,
            total_stars: 0,
        }
    }
}
impl GaiaCatalogue {
    /// Publish a completed snapshot atomically, retaining the last one while loading.
    /// Movement can outrun a query: publish its progress anyway, then catch up with
    /// another query. Rendering still evaluates positions and flux at the current camera.
    fn refresh_selection(
        &mut self,
        origin: GalacticPosition,
        cutoff: f64,
        completed: Option<Selection>,
    ) -> bool {
        if let Some(selection) = completed.filter(|s| s.cutoff == cutoff) {
            info!(
                "Gaia: {} cached / {} matching sources, {:.2} ms query",
                selection.stars.len(),
                selection.matched,
                selection.seconds * 1000.0
            );
            self.revision += 1;
            self.selected = selection.stars.len();
            self.matched = selection.matched;
            self.seconds = selection.seconds;
            self.candidates = selection.candidates;
            self.buckets_searched = selection.buckets_searched;
            self.total_stars = selection.total_stars;
            self.current = Some(selection);
            self.state = if self.matched > self.selected {
                "Ready (brightest stars capped)"
            } else {
                "Ready"
            }
            .into();
        }
        self.current
            .as_ref()
            .is_none_or(|s| s.cutoff != cutoff || origin.relative_to(s.origin).length() > s.budget)
    }
    pub fn nearest_distance(&self) -> f64 {
        self.current
            .as_ref()
            .map_or(f64::INFINITY, |s| s.budget * 100.0)
    }
    pub fn stars(&self) -> &[Star] {
        self.current.as_ref().map_or(&[], |s| s.stars.as_slice())
    }
}
pub struct GaiaPlugin;
impl Plugin for GaiaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GaiaCatalogue>()
            .add_systems(Update, stream);
    }
}
fn stream(
    catalogue: Option<ResMut<GaiaCatalogue>>,
    settings: Option<Res<SkySettings>>,
    camera: Query<&PreciseTransform, With<MainCamera>>,
) {
    let (Some(mut field), Some(settings), Ok(camera)) = (catalogue, settings, camera.single())
    else {
        return;
    };
    if field.failed {
        return;
    }
    if field.reader.is_none() && field.opening.is_none() {
        field.opening = Some(
            AsyncComputeTaskPool::get().spawn(async { StarCatalogue::embedded().map(Arc::new) }),
        );
    }
    if let Some(task) = &mut field.opening {
        if let Some(result) = check_ready(task) {
            field.opening = None;
            match result {
                Ok(reader) => {
                    field.total_stars = reader.len();
                    field.reader = Some(reader);
                }
                Err(error) => {
                    field.state = format!("Error: {error:#}");
                    field.failed = true;
                    return;
                }
            }
        }
    }
    let origin = camera.translation_um;
    // Moving 1% of the nearest-star distance changes magnitude by at most
    // 0.022; 0.05 magnitude headroom safely covers the cached interval.
    let cutoff = min_brightness(settings.0.magnitude_limit + 0.05);
    let mut completed = None;
    if let Some(task) = &mut field.task {
        if let Some(result) = check_ready(task) {
            field.task = None;
            match result {
                Ok(selection) => completed = Some(selection),
                Err(error) => {
                    field.state = format!("Error: {error:#}");
                    field.failed = true;
                    return;
                }
            }
        }
    }
    if field.refresh_selection(origin, cutoff, completed) {
        if field.task.is_none() {
            if let Some(reader) = field.reader.clone() {
                field.task = Some(AsyncComputeTaskPool::get().spawn(async move {
                    select(&reader, origin, cutoff, MAX_SELECTED_STARS, &[])
                }));
                field.state = if field.current.is_some() {
                    "Refreshing"
                } else {
                    "Selecting"
                }
                .into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> GaiaCatalogue {
        GaiaCatalogue::default()
    }

    fn snapshot(x: i128, cutoff: f64, id: i64) -> Selection {
        Selection {
            stars: vec![Star {
                id: StarId::gaia(id as u64),
                position: GalacticPosition::new(0, 0, 0),
                luminosity: 1.,
                colour: [1.; 3],
            }],
            origin: GalacticPosition::new(x, 0, 0),
            budget: 1., // Metres; positions are micrometres.
            seconds: 0.,
            matched: 1,
            cutoff,
            candidates: 1,
            total_stars: 1,
            buckets_searched: 1,
        }
    }

    #[test]
    fn refresh_keeps_stars_visible_and_continuous_movement_makes_progress() {
        let mut field = catalogue();
        let start = GalacticPosition::new(0, 0, 0);
        assert!(field.refresh_selection(start, 1., None));
        assert!(field.stars().is_empty());
        assert!(!field.refresh_selection(start, 1., Some(snapshot(0, 1., 1))));

        for step in 1..10 {
            let x = step * 10_000_000;
            let camera = GalacticPosition::new(x, 0, 0);
            // Multiple frames pass while the background query is outstanding.
            for _ in 0..3 {
                assert!(field.refresh_selection(camera, 1., None));
                assert_eq!(field.stars()[0].id.value, step as u64);
            }
            // A query finishes already outside its movement budget. Still publish
            // it and request the next snapshot, rather than starving the display.
            assert!(field.refresh_selection(
                camera,
                1.,
                Some(snapshot(x - 5_000_000, 1., step as i64 + 1)),
            ));
            assert_eq!(field.stars()[0].id.value, step as u64 + 1);
        }
        let stopped = GalacticPosition::new(90_000_000, 0, 0);
        assert!(!field.refresh_selection(stopped, 1., Some(snapshot(90_000_000, 1., 20))));
        assert_eq!(field.stars()[0].id.value, 20);
        assert!(!field.refresh_selection(stopped, 1., None));
    }

    #[test]
    fn settings_change_retains_display_but_rejects_obsolete_query() {
        let mut field = catalogue();
        let camera = GalacticPosition::new(0, 0, 0);
        field.refresh_selection(camera, 1., Some(snapshot(0, 1., 1)));
        assert!(field.refresh_selection(camera, 2., None));
        assert_eq!(field.stars()[0].id.value, 1);
        assert!(field.refresh_selection(camera, 2., Some(snapshot(0, 1., 2))));
        assert_eq!(field.stars()[0].id.value, 1);
        assert!(!field.refresh_selection(camera, 2., Some(snapshot(0, 2., 3))));
        assert_eq!(field.stars()[0].id.value, 3);
    }
}
