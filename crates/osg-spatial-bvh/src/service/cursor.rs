use super::query;
use super::{IndexedTree, SpatialObject, SpatialService};
use crate::luminosity_bvh::NodeKind;
use crate::{Aabb, Position};

/// Candidate queries available through a metered, resumable traversal.
/// All variants use instantaneous bounds except Motion, which uses swept bounds.
#[derive(Clone, Copy, Debug)]
pub enum SpatialQuery {
    Sphere {
        centre: Position,
        radius_m: f64,
    },
    Aabb(Aabb),
    Visibility {
        observer: Position,
        observer_radius_m: f64,
        min_flux_w_m2: f64,
    },
    Segment {
        start: Position,
        end: Position,
        radius_m: f64,
    },
    Motion(Aabb),
}

impl SpatialQuery {
    fn validate(self) {
        match self {
            Self::Sphere { radius_m, .. } | Self::Segment { radius_m, .. } => {
                query::validate_radius(radius_m);
            }
            Self::Aabb(bounds) | Self::Motion(bounds) => {
                crate::luminosity_bvh::validate(bounds, 0.0);
            }
            Self::Visibility {
                observer_radius_m,
                min_flux_w_m2,
                ..
            } => {
                query::validate_radius(observer_radius_m);
                assert!(
                    min_flux_w_m2.is_finite() && min_flux_w_m2 >= 0.0,
                    "flux threshold must be finite and nonnegative"
                );
            }
        }
    }

    fn matches(self, bounds: Aabb, luminosity: f64) -> bool {
        match self {
            Self::Sphere { centre, radius_m } => query::within_distance(bounds, centre, radius_m),
            Self::Aabb(query) | Self::Motion(query) => query::overlaps(bounds, query),
            Self::Visibility {
                observer,
                observer_radius_m,
                min_flux_w_m2,
            } => query::visible(
                bounds,
                luminosity,
                observer,
                observer_radius_m,
                min_flux_w_m2,
            ),
            Self::Segment {
                start,
                end,
                radius_m,
            } => query::segment_entry(bounds, start, end, radius_m).is_some(),
        }
    }
}

/// Limits for one advance. Zero work or zero results pauses without advancing.
#[derive(Clone, Copy, Debug)]
pub struct QueryBudget {
    pub max_work: usize,
    pub max_results: usize,
}

/// Traversal work, including rejected nodes and rejected object records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryStats {
    pub nodes_visited: usize,
    pub objects_tested: usize,
}

impl QueryStats {
    /// One unit per visited node and per tested object. Caller-side exact
    /// predicates, sorting, and serialization require separate accounting.
    pub fn work(self) -> usize {
        self.nodes_visited + self.objects_tested
    }
}

/// A batch in traversal order, with statistics for this advance only.
/// An incomplete batch must not be treated as a complete answer.
#[derive(Debug)]
pub struct QueryBatch<'a, T> {
    pub objects: Vec<&'a T>,
    pub stats: QueryStats,
    pub complete: bool,
}

#[derive(Clone, Copy)]
struct Pending {
    dynamic: bool,
    index: usize,
    object: bool,
}

/// A traversal borrowing the current service. Construction validates the
/// query immediately. Statistics are cumulative across calls to advance.
pub struct QueryCursor<'a, T: SpatialObject> {
    service: &'a SpatialService<T>,
    query: SpatialQuery,
    pending: Vec<Pending>,
    stats: QueryStats,
}

impl<'a, T: SpatialObject> QueryCursor<'a, T> {
    pub(super) fn new(
        service: &'a SpatialService<T>,
        query: SpatialQuery,
        include_static: bool,
    ) -> Self {
        query.validate();
        let mut pending = Vec::new();
        for (dynamic, root) in [
            (true, dynamic_tree(service, query).root),
            (false, service.static_tree.root.filter(|_| include_static)),
        ] {
            if let Some(index) = root {
                pending.push(Pending {
                    dynamic,
                    index,
                    object: false,
                });
            }
        }
        Self {
            service,
            query,
            pending,
            stats: QueryStats::default(),
        }
    }

    pub fn stats(&self) -> QueryStats {
        self.stats
    }

    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }

    /// Perform at most max_work node/object tests and return at most max_results
    /// candidates. Rejected work also consumes budget. Resume even when a batch
    /// has no objects if complete is false.
    pub fn advance(&mut self, budget: QueryBudget) -> QueryBatch<'a, T> {
        let mut batch = QueryBatch {
            objects: Vec::new(),
            stats: QueryStats::default(),
            complete: false,
        };
        while batch.stats.work() < budget.max_work && batch.objects.len() < budget.max_results {
            let Some(pending) = self.pending.pop() else {
                break;
            };
            if pending.dynamic {
                step(
                    dynamic_tree(self.service, self.query),
                    self.query,
                    pending,
                    &mut self.pending,
                    &mut batch,
                );
            } else {
                step(
                    &self.service.static_tree,
                    self.query,
                    pending,
                    &mut self.pending,
                    &mut batch,
                );
            }
        }
        batch.complete = self.pending.is_empty();
        self.stats.nodes_visited += batch.stats.nodes_visited;
        self.stats.objects_tested += batch.stats.objects_tested;
        batch
    }

    /// Complete the remaining traversal without a work or result limit.
    /// Objects returned by earlier advances are not returned again.
    pub fn collect(mut self) -> Vec<&'a T> {
        self.advance(QueryBudget {
            max_work: usize::MAX,
            max_results: usize::MAX,
        })
        .objects
    }
}

fn dynamic_tree<T: SpatialObject>(
    service: &SpatialService<T>,
    query: SpatialQuery,
) -> &IndexedTree<T> {
    match query {
        SpatialQuery::Motion(_) => &service.swept_tree,
        _ => &service.dynamic_tree,
    }
}

fn step<'a, T: SpatialObject>(
    tree: &'a IndexedTree<T>,
    query: SpatialQuery,
    pending: Pending,
    stack: &mut Vec<Pending>,
    batch: &mut QueryBatch<'a, T>,
) {
    let node = &tree.nodes[pending.index];
    if pending.object {
        let NodeKind::Leaf(id) = &node.kind else {
            unreachable!("pending object must be a leaf")
        };
        let record = &tree.records[id];
        batch.stats.objects_tested += 1;
        batch.objects.push(record);
        return;
    }

    batch.stats.nodes_visited += 1;
    if !query.matches(node.bounds, node.max_luminosity) {
        return;
    }
    match &node.kind {
        NodeKind::Leaf(_) => stack.push(Pending {
            object: true,
            ..pending
        }),
        NodeKind::Branch { left, right } => {
            stack.push(Pending {
                index: *right,
                ..pending
            });
            stack.push(Pending {
                index: *left,
                ..pending
            });
        }
    }
}
