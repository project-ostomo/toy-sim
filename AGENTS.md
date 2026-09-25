# Development policy

This project is in early prototyping. Prefer clean changes to preserving old
interfaces or behavior for compatibility.

Do not add compatibility shims, compatibility layers, legacy adapters, or
backward-compatibility paths unless the user explicitly requests them. Update
callers, examples, tests, and bundled assets to the new interface directly.
Remove obsolete interfaces instead of retaining them behind an adapter.

# Code style

`pub(crate)` and `pub(super)` are banned. Use either `pub` or private visibility.
When a submodule's types need to be shared with the wider crate, its parent
module must re-export them with `pub use`. If this makes the code awkward,
rethink the module structure.

Place inherent `impl SomeType { ... }` blocks immediately after the type's
definition. Keep its methods together there rather than spreading inherent
implementations across modules.

Use scheduled ECS systems with explicit queries and resources for simulation
actions. Keep an object's interior state in ordinary structs owned by its
component when that state has no independent ECS lifecycle. Put local invariants
and state transitions in methods on those structs.

Use typed FIFO queue resources for ordered requests and consume each queue in
one system. Validate and apply a complete action within that system. Declare
phase ordering in the schedule. Avoid runtime helpers taking `&mut World`,
temporarily removing domain components, and deferred callbacks that implement
domain logic. World access remains appropriate at bootstrap, persistence, and
network transport boundaries. Tests should exercise queues and the registered
production schedules.

Prefer ordinary schedule boundaries for initialization, discovery, and publication.
An additional frame or simulation tick is acceptable when no correctness invariant
requires immediate consumption. Add explicit ordering only for a concrete data or
behavioral dependency, and document that dependency when it is not obvious. Use
Bevy's automatic deferred-command synchronization for those dependencies; an
explicit `ApplyDeferred` needs a specific reason. Avoid chaining independent work.

For monetary charges, round each charge up to the smallest currency unit.
Do not implement fractional accumulation, carried remainders, or rounding-debt
bookkeeping unless explicitly requested. This applies to turnover tax and
demurrage as well as other charges.

Write readable code with normal spacing, blank lines between logical steps, and
multi-line functions where appropriate. Do not write compressed code and rely on
rustfmt to make it readable; rustfmt does not supply logical separation.

# Testing and verification

Do not write tests for reversible, low-impact changes that mirror the implementation. If you do choose to verify your work with tests, make sure that the tests are meaningful and necessary to verify implementation.

Run tests appropriate to the change and complete required checks. Once those pass, broaden or repeat testing only when new changes, failures, or unresolved concerns justify it; otherwise, continue toward completing the task.

# UI verification and refinement

Default to headless, software-rendered screenshots and targeted checks for UI
verification. Do not drive the live desktop with mouse or keyboard input,
including clicks and drags, unless the user explicitly requests it.

Limit UI refinement to the requested requirements and concrete failures. Stop
once the relevant checks pass; do not continue cosmetic tweaking or repeated
visual refinement unless the user specifically asks for it.

## (Codex) File editing

For manual source-code edits, MUST use Codex's native `apply_patch` tool.

Do not modify files by writing Python, Perl, Ruby, sed, awk, cat,
heredocs, or other shell scripts when `apply_patch` can reasonably
perform the edit.

You *should* use bash tools like "rm" and "mv" for things like moving files and deleting files, rather than applying huge patches.

Python/scripts are allowed only for:
- genuinely generated output,
- large mechanical/bulk transformations,
- edits where scripting is materially safer or more reliable,
- or after `apply_patch` has failed.

Prefer several small `apply_patch` calls over a Python rewrite.

## No slop writing

Avoid using slop words or phrases like "Bottom Line:" in conclusions, "delve," "foster," "leverage," "it's worth noting," "importantly," "Question? Answer." or "This isn't about X. It's about Y.", "genuinely" or hyphenated compound descriptions and adjectives. Do not use concluding summary statements such as "In short:..", "The simplest mental model is:...".

State the intended action directly. Avoid adding what you won't do, what will remain unchanged, or how you'll separate or categorize results. Do not use contrastive framing such as "X, not Y" or "X—not Y" that introduces an unprompted alternative that the user didn't ask about. Avoid invented compound labels like "exact-head checks" and "editorial-row layouts", vague qualifiers, and canned transitions; use plain verbs and prepositions to state the actual relationship directly.
