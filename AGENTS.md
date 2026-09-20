# Development policy

This project is in early prototyping. Prefer clean changes to preserving old
interfaces or behavior for compatibility.

Do not add compatibility shims, compatibility layers, legacy adapters, or
backward-compatibility paths unless the user explicitly requests them. Update
callers, examples, tests, and bundled assets to the new interface directly.
Remove obsolete interfaces instead of retaining them behind an adapter.

# Code style

Write readable code with normal spacing, blank lines between logical steps, and
multi-line functions where appropriate. Do not write compressed code and rely on
rustfmt to make it readable; rustfmt does not supply logical separation.

# English documentation

Codex must delegate writing or rewriting English documentation to Claude Code
through the `claude` CLI, except for the credit-limit fallback below. This includes
README files, guides, architecture notes, and English documentation added during
code changes. Codex must not draft
or rewrite that prose itself. Codex may inspect implementation details, give
Claude Code requirements and factual corrections, and verify its output. Any
prose corrections must also be delegated to Claude Code.

If Claude Code reports that it is out of credits or has reached a spending limit,
Codex must continue writing, rewriting, and correcting the documentation directly.
No additional user approval is needed for this fallback. For other failures or
unavailability, report the blocker instead of writing the documentation directly.
This rule applies to repository documentation; ordinary conversation with the
user is exempt.

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

Python/scripts are allowed only for:
- genuinely generated output,
- large mechanical/bulk transformations,
- edits where scripting is materially safer or more reliable,
- or after `apply_patch` has failed.

Prefer several small `apply_patch` calls over a Python rewrite.