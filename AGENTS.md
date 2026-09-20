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

You *should* use bash tools for things like moving files and deleting files, rather than applying huge patches.

Python/scripts are allowed only for:
- genuinely generated output,
- large mechanical/bulk transformations,
- edits where scripting is materially safer or more reliable,
- or after `apply_patch` has failed.

Prefer several small `apply_patch` calls over a Python rewrite.