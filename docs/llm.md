# Asynchronous language-model calls

The server provides one asynchronous LLM service for ship programs and NPC
directors. Real OpenRouter access requires both `[llm] enabled = true` in the
server configuration and `OPENROUTER_API_KEY` in its environment. The debug
launcher enables the configuration with `--enable-llm`. Ordinary launches and
automated test servers leave it disabled. Unit tests use an injected fake
provider and cannot enable the real provider through the environment.

The current provider is `z-ai/glm-5.3`. Requests contain a fixed ship-computer
instruction and the supplied prompt. The service supplies no hidden world state,
files or external tools. Callers must build their context from information they
are authorized to observe. Provider output is text; game actions remain subject
to the ordinary authoritative APIs.

## Admission and polling

The shared model defines `LlmRequest { id, prompt, max_tokens }`. IDs are positive
integers; prompts contain at most 32 KiB of UTF-8 text; output limits range from
1 to 2,048 tokens. Programs should retain their request counter in committed
durable data. The server derives the caller's actual owner, computer UUID,
program digest and flight/display distinction. These values scope every request
ID and result. Missile callbacks share their parent flight program's scope.

`llm_submit` returns a small admission status immediately. Programs use
`llm_poll` to obtain `Pending`, `Ready`, `Failed`, `Cancelled`, `Indeterminate` or
`Unknown`. Visible result text is limited to 64 KiB. A cached request with the
same ID and payload returns `AlreadyKnown`; changing its payload is rejected.
Durable request records also prevent an evicted or restored request from being
sent to the provider again. A new attempt requires a new request ID.

The service admits at most 64 pending requests, eight concurrent provider calls,
four pending requests per owner and two per physical computer. It retains 1,024
results in memory. Older results can be unavailable to polling; resubmitting the
same request recovers its durable status without another provider call. The
ledger retains at most 100,000 request identities and trims old large responses.
Those limits produce explicit admission or result statuses.

The host calls consume ordinary metered CPU gas for validation and copying.
Accepted service submissions also immediately debit their owner's global gas
account by `1,000,000 + 100 × prompt_bytes + 1,000 × max_tokens`. This fixed quote
is a gameplay charge for admission. It is settled before the asynchronous work
starts, so a world checkpoint never waits for an outstanding LLM gas reservation.
A cached duplicate does not pay it again. An evicted request submitted again
pays admission gas even when the durable ledger recovers an existing result.

SQLite operations and network waits run on a separate worker. The worker sends
bounded completions back to the service; polling or draining applies them to
the caller-visible result cache. It never mutates ships, inventories, orders or
other world state.

## Durable dollar budget

The entire installation shares a $100 cap stored separately from world saves:
`$XDG_DATA_HOME/toy-sim/llm-spend.sqlite`, or
`~/.local/share/toy-sim/llm-spend.sqlite` when `XDG_DATA_HOME` is unset. Starting
another world or restoring an older world checkpoint does not reset this ledger.
SQLite uses WAL mode, full synchronization and immediate transactions to reserve
money atomically across processes. The ledger counts settled charges plus every
unsettled reservation against the cap.

Before sending a request, the service commits a conservative dollar reservation.
Provider routing limits prices to $2 per million input tokens and $8 per million
output tokens, with zero per-request or image fees. Model and provider fallbacks,
HTTP retries and redirects are disabled. No tool or plugin can add another
billable operation. The prompt allowance is twice its UTF-8 byte length,
including the fixed instruction, plus 8,192 tokens. The output allowance is
twice the requested token limit plus 1,024 tokens. This padding covers formatting
overhead and small token-limit overruns; enforcement also depends on OpenRouter
honoring its advertised price and token controls.

OpenRouter's `max_tokens` generally includes reasoning and visible output
together. A reply can therefore contain no visible text while still incurring
a charge. The service requests low reasoning effort, records the provider's
reported usage cost and rounds it upward to whole microdollars using decimal
integer arithmetic. See the official [provider price controls](https://openrouter.ai/docs/guides/routing/provider-selection#max-price)
and [reasoning token accounting](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens#reasoning-tokens-and-max_tokens).

A timeout, transport failure, malformed response or missing billing information
leaves the entire reservation held. After a crash, unfinished requests become
`Indeterminate`; recovery never sends them again or assumes they were free.
Known charges and terminal results settle atomically and idempotently. A charge
above the reserved bound stops the worker and further admission visibly.

Cancelling a queued request can prevent dispatch. Cancelling a request already
sent discards its eventual visible result while the worker still settles its
known charge, or retains the reservation if billing is uncertain. Cancellation
does not refund admission gas. A durable result recovered after cache eviction
remains the original result; cancelling the duplicate lookup cannot cancel a
previously completed provider operation.

Admission and completion logs contain the request ID, computer UUID, reservation
or charge, and aggregate dollar totals. Provider failures log a safe failure
category or HTTP status. API keys, prompts and response bodies are never logged.
