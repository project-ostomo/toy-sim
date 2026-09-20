# Server, routing, and navigation performance plan

## Outcomes

- Keep the complete default-world server loop below the 100 ms budget for 10 Hz, with useful headroom, including publication.
- Return ordinary route previews within one second; target a two-second computation deadline and prompt cancellation. These are acceptance targets to measure, not claims about current performance.
- Show inhabited-system sovereignty derived from the organization owning a strict majority of its publicly broadcasting installations.
- Preserve physical activation, client-generated celestials, authoritative ship physics, whole-itinerary destruction risk, fuel accounting, and unrestricted controller aim.

## Evidence and constraints

The last detailed spatial profile measured approximately 94 ms: 33 ms inserting 46,893 objects, 49 ms selecting illumination sources, and 11 ms calculating reflections. Approximately 3,000 objects were ships or installations. These measurements precede the latest celestial visibility correction. Hardware previously cost 29–35 ms, activation 18–20 ms, and optimized celestial movement approximately 3.5 ms. Concurrent phase timings must not be added as though they were serial wall time.

The router's `routing/graph.rs` queue uses cost already spent. It restarts for every maximum leg count from 1 through 32, increasing candidate limits from 64 to 512. Each candidate can invoke iterative departure clearance and a slip forecast before the search reaches more promising states. Candidate selection currently samples systems near the origin and goal. A heuristic alone will not remove expensive edge evaluation or repeated searches.

`InhabitedDirectory` currently contains only system IDs. The client initializes `NavigationSystem.sovereignty` to `None`; existing map labels, colors, and filters consequently have no ownership information to use.

Work in this plan should begin after the current manual playtest. Capture a fresh baseline before modifying runtime code. Avoid adding a new general profiling framework.

## 1. Publish inhabitation and sovereignty together

Use the same eligibility rule as public inhabitation: an installation has an operational directory transmitter, an enabled transponder, and a physical presence within the system's influence. Count each installation once per containing system, regardless of the number of transmitter modules. Ships without that installation equipment and dark installations do not vote. Navigation assistance equipment is independent of this rule.

Resolve votes from authoritative `ownership::AssetOwner`, never an advertised transponder faction. For organization-owned assets, count votes for that organization and resolve its sovereignty through `OwnershipDirectory.organizations`. For player-owned assets, use the player's current organization through the existing ownership lineage. Unaffiliated or directly sovereignty-owned assets contribute to inhabited status and the total installation count but do not manufacture an organization vote.

An organization wins only with more than half of all eligible installations. Publish that organization's sovereignty as system ownership. A tie, a plurality below half, or no eligible organization produces an unclaimed system. Organizations belonging to the same sovereignty are not pooled under this rule. These edge-case rules are explicit implementation defaults.

Update the result when ownership, affiliation, transmitter power, transponder state, destruction, or influence membership changes. Removing the final broadcast removes the inhabited entry entirely. Orbital pose changes alone must not invalidate ownership assets when membership is unchanged.

Implementation:

- `crates/osg-server/src/sim/infrastructure/navigation.rs`: derive membership and organization tallies in the same pass; publish one result for session and route-service consumers.
- `crates/osg-server/src/sim/ownership.rs`: use `AssetOwner` and the existing directory lineage; account for ownership and affiliation updates.
- `crates/osg-model/src/presentation.rs`: extend the inhabited directory with a compact sorted ownership mapping, preferably system IDs plus sovereignty IDs and a deduplicated sovereignty table. Avoid repeating sovereignty names for every system.
- `crates/osg-protocol/src/navigation.rs`: update asset version, validation, and size limits for the actual encoded representation. Preserve support for a million inhabited IDs and bounded asset retention; update callers directly.
- `crates/osg-client/src/assets.rs`, `state/mod.rs`, and `ui/shell/map/{layout,canvas}.rs`: join ownership onto locally generated system metadata. Ensure ownership-only changes invalidate map search/filter caches. Existing `map.rs` sovereignty labels and filters should consume the populated values.
- Supply public sovereignty names and bloc metadata needed by the map independently of any society-screen permission filtering. Never stream celestial definitions for this purpose.

## 2. Replace repeated route exploration with goal-directed search

### Search and candidates

Keep the catalogue as the broad search structure. Generate neighbors lazily from compact system summaries, always including the requested destination. Include useful intermediate systems along the remaining journey, nearby alternatives, and accessible assisted destinations. Widen candidate coverage within the request budget when needed; do not construct a graph of the entire sky or activate searched systems.

Use A* ordering `f = g + h`, with `g` matching the existing time-plus-weighted-propellant objective. Start with a cheap optimistic travel-time estimate from remaining distance and the maximum supported slip speed, accounting for capture radii and the possibility of an ordinary-flight finish. Establish that a bound covers all permitted motion before calling it admissible. If a stronger practical estimate is used, treat the result as a bounded heuristic route search and make no global optimality claim. Sparse candidate selection already limits completeness.

Try direct travel and a small set of promising assisted alternatives early to obtain a validated incumbent. Stop when the search proves no better route exists in its searched graph or the interactive budget expires. Return a validated incumbent at the deadline, with a bounded-search status; never return an unvalidated candidate as executable.

Eliminate the 32 independent full searches. Carry resource usage in search labels and choose a small set of leg-risk allocations based on remaining itinerary risk and estimated remaining legs. Include alternative allocations when widening the search, rather than restarting all discovery. Retain the current maximum itinerary length and validate exact cumulative log loss and fuel on the selected route.

Dominance pruning must account for propellant, exotic fuel, risk, and remaining leg allowance. Reaching the same system more cheaply does not establish dominance if it spends more of another resource. Arrival time and pose also affect future edges; do not collapse physically different states without a justified approximation. Bound retained labels explicitly and describe search exhaustion honestly.

### Make rejected edges cheap

Order work as follows:

1. Summary geometry, duplicate/cycle rejection, dispersion-floor feasibility, optimistic cost, and conservative fuel bounds.
2. Speed/risk choices and ranking of promising neighbors.
3. Exact departure clearance, time-dependent target resolution, and collision/capture forecast when that edge is about to matter to the search.
4. Exact validation of the complete selected itinerary before publication.

Bounds must be conservative: a center-to-center distance is not automatically a lower bound after departure clearance and capture geometry. Cache catalogue candidates freely; cache physical edge results only with all relevant pose, time, ship-performance, permission, and world-snapshot inputs. A small request-local cache is preferable to a global invalidation subsystem.

Implementation:

- `crates/osg-server/src/sim/routing/graph.rs`: queue priority, predecessor reconstruction instead of copying full paths, resource labels, incumbent, and staged edge evaluation.
- `crates/osg-server/src/sim/routing/mod.rs`: search budget, route validation, risk-allocation policy, and outcome semantics.
- `crates/osg-server/src/sim/services/route_environment.rs`: goal-directed catalogue candidates and cheap summary queries; detailed generation only for evaluated geometry.
- `crates/osg-server/src/sim/route_service.rs`: wall-clock deadline and cancellation checks inside costly loops; bounded worker concurrency; distinguish admission wait from execution time. Superseded preview requests should release worker capacity promptly.
- `crates/osg-model/src/routing.rs`, `crates/osg-protocol/src/routing.rs`, and `crates/osg-client/src/ui/shell/map/planner.rs`: represent ready, searching, cancelled, budget-exhausted, and validated bounded-search results clearly. A bounded search failure is not proof of physical unreachability.

Routing remains advisory. None of these checks may become new server admission vetoes on controller aim.

## 3. Shrink the observation index

Keep detectable gameplay objects in one dynamic spatial index. Store celestial geometry in compact per-system lists using existing active definitions and current orbital poses. Query the universe catalogue for systems intersecting the actual observation sphere or sightline, then apply exact celestial sphere predicates. Cover overlapping influence regions and objects between systems.

Remove the observation index's `Arc`/copy-on-write wrapper: no production consumer currently retains a clone. Reuse buffers. After reducing entries, compare the existing tree against a compact rebuild and flat scans for small query populations. Do not introduce thousands of tiny trees.

Implementation: `sim/spatial.rs`, `sim/spatial/index.rs`, `sim/orrery/activity.rs`, and catalogue query helpers in `crates/osg-universe/src/catalogue.rs`.

Keep the static catalogue index and swept collision index where their distinct semantics require them. Sharing geometry must preserve phase freshness, precise coordinates, object extents, and continuous collision detection.

## 4. Evaluate optical visibility on demand

Celestials remain universally known. Compute exact target illumination and stellar shadowing only for candidates requested by actual visibility consumers, including intelligence acquisition and optical session publication. Cache target illumination once per tick; apply viewing phase and line-of-sight checks per observer.

Candidate selection must include reflected light. Use a conservative incident-irradiance bound per region and target cross-section to reject impossible detections before expensive lighting. Do not invent a finite optical range or select candidates using emitted light alone. A cold sunlit ship must remain detectable.

Select stellar sources from shared catalogue data, so distant illumination does not depend on unrelated server activation. Handle companion stars and authored systems conservatively, and bound omitted contributions. Detailed source evaluation must not activate a system.

Implementation: `sim/spatial/lighting.rs`, `sim/spatial/index.rs`, `sim/intelligence.rs`, and `sim/session/optical.rs`.

## 5. Remove duplicate publication work and remaining hot paths

Publish current membership once at a defined simulation boundary. Reuse activation overlap candidates, refined against current position, rather than independently repeating containment searches in navigation and services. Preserve pose freshness and separate changing poses from ownership/directory topology. The existing directory hash already changes only when membership changes; extend that logic to ownership.

Inspect the two aperture indexes in `sim/services.rs`. Consolidate only where query semantics and snapshot lifetimes permit; apply visibility permissions before result limits. For `sim/chat.rs`, compare scanning endpoints on message delivery against maintaining a tree every tick.

Measure hardware allocation and repeated design/state scans before introducing scheduling changes. Preserve fuel exhaustion, power loss, thermal thresholds, and computer execution timing. Preserve existing gravity and celestial movement improvements. Measure checkpoint capture separately from ordinary ticks and optimize its demonstrated copying or serialization bottleneck if it causes stalls.

Removing celestial ECS entities entirely is a possible later simplification. It is not required for the first performance pass and would require migrating every gravity, collision, atmosphere, and reference consumer.

## Delivery and verification

Implement in reviewable batches: shared publication plus sovereignty; route search; smaller spatial index; demand-driven optical work; measured remaining bottlenecks. Capture timings after each batch and stop tuning when the agreed workloads meet the targets.

Use existing focused checks and add coverage only for changed behavior:

- Majority ownership, ties, affiliation/ownership transfer, dark or destroyed installations, and membership removal. Verify navigation labels, colors, and filters with a headless screenshot.
- Direct, assisted, multi-leg, dispersion-floor-impossible, fuel-limited, and unreachable routes. Verify cumulative ppm risk, exact itinerary validation, prompt cancellation, and a validated incumbent on deadline. Compare small deterministic graphs against exhaustive solutions where optimality is claimed.
- Existing shadows, reflected cold targets, occlusion, nearest contacts, and cross-system boundary geometry; compare changed broad-phase query results against brute force on small representative worlds.
- Default seeded world, crowded system, multiple observers, and simultaneous route previews. Record complete-loop median/p95/max, publication time, route queue delay, route execution time, expanded labels, and exact forecasts. Include checkpoint stalls separately.

Do not resolve failures by raising client timeouts, reducing physical fidelity, weakening risk checks, hiding inhabited systems, or silently discarding valid visibility candidates. Record any remaining performance limit and its measured cause.

## Implementation measurements

The implemented search uses weighted A* ordering, lazy edge forecasts, request-local authorized-beacon indexing, bounded alternative risk allocations, and a two-second computation deadline. It returns validated feasible routes and does not claim global optimality.

The seeded-world benchmark runs 120 ticks and reports ticks 60–119, after ordinary computer boot. Including navigation publication and an optical observer, it measured 88.69 ms median, 92.68 ms p95, and 93.78 ms maximum. Cold initialization is excluded from these steady-state figures.

Seeded route requests at the default 100 ppm allowance completed in 301.55 ms at 7.925 ly, 280.78 ms at 10.114 ly, and 340.97 ms at 100.010 ly. The longest route used two slip legs. Exact validation confirmed fuel and cumulative risk allowances.

The previously failing authenticated display and process-restart integration tests pass with their original timeouts. Focused checks also cover ownership-only directory updates, public sovereignty labels with an empty private society directory, inactive stellar illumination, occlusion, spatial query bounds, routing, and chat delivery.

Checkpoint capture now reuses serialized immutable blueprints and default firmware hashes within each capture. The restart integration test measured approximately 70 ms of capture time, down from approximately 350 ms, and passed with its original timeout. Database writing remains on the existing background worker. Celestial ECS removal and hardware scheduling changes remain unnecessary for this performance pass; the current simulation timing and collision model are preserved.
