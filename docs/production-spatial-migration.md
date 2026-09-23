# Production spatial migration

Implementation of the agreed production plan, September 2026.

- [ ] Conservative spatial hash queries, nearest surfaces, and segment traversal.
- [ ] Catalogue, celestial, star, and production spatial consumers.
- [ ] Persistent production index with 512 km minimum cells.
- [ ] Production Rapier groups and one 100 ms gameplay tick.
- [ ] Production weapons, thermal damage, shields, and lifecycle systems.
- [ ] Controllers, services, travel, economy, sessions, and persistence ownership.
- [ ] Remove obsolete BVH and collision solver interfaces.
- [ ] Correctness checks and complete-server measurements.

Runtime ordering is restricted to producer/consumer dependencies. Independent
systems may run in either order. Physics state belongs to ECS; Rapier receives
local group inputs and returns motion and contact facts. Celestials retain
analytic motion and their existing collision participation policy.

Static catalogue records are seeded once. Moving records update in place.
Nearest-surface queries must prove completeness using the maximum object radius;
exhausted query budgets must never imply a clear path. Celestial envelope queries
retain exact checks against moving ephemerides.

Gameplay runs once per 100 ms, with internal Rapier CCD subdivisions allowed.
Due shots are batched at boundaries, preserving rates and resource spending.
The production implementation must test the prototype's delayed CCD response,
principal-inertia frame conversion, and contact episode accounting.
