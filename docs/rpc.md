# Client networking and RPC

`osg_net::OsgNetClient` owns an authenticated picomux connection. Clone it to
share the connection between UI tasks or headless agent tasks. Its methods work
when awaited from Bevy's task pool; network work and deadlines run on the Tokio
runtime used to connect.

```rust
let client = OsgNetClient::connect(address, server_key, account, &secret).await?;
let mut events = client.subscribe_events();

// After receiving a session descriptor:
let balance = client.wallet_balance(world, owner).await??;
let history = client.wallet_history(world, owner, None, 100).await??;
client.transfer_money(operation, owner, recipient, Currency::Uec, amount).await??;
```

## API

[GameRpc](../crates/osg-net/src/game.rs) declares the Rust call signatures.
The `osg-net-macros` attribute generates inherent methods on `OsgNetClient`
and a server dispatcher for picomux streams.

Calls describe individual functions: wallet balances and history, money and gas
transfers, order books, orders and trades, asset searches and goods totals,
inventory and hangar access, industry jobs, identity records, permissions,
and diplomacy. Clients compose the data needed by each screen. Navigation route
search runs in the client.
Shared records and `Page<T, Cursor>` supply result data. Screen query state and
internal domain command enums are not RPC request envelopes.

Mutation return values are `Result<(), GameError>`. Queries return their declared
data type, usually inside `Result<T, GameError>`. The generated client adds an
outer `Result<_, RpcError>` for local transport and protocol failures. Thus a
mutation's client signature returns `Result<Result<(), GameError>, RpcError>`.

## Wire format

RPC uses one picomux stream per call:

1. Open a stream with UTF-8 metadata `rpc:<method_name>`.
2. Serialize the argument tuple with Postcard, including a one-element tuple for
   a single argument and `()` for zero arguments.
3. Send a four-byte big-endian byte length followed by the encoded arguments.
   Close the stream's write direction.
4. Receive a four-byte big-endian byte length and the Postcard encoding of the
   declared return value, followed by EOF.

The return value is serialized directly. There is no framework error envelope.
Unknown methods, malformed messages, extra bytes, and framework failures close
the individual stream. The server logs the failure. Other streams remain usable.

Requests are limited to 64 KiB, replies to 8 MiB, and active RPCs to 16 per
connection. The deadline is 30 seconds. Calls are never retried automatically.
Dropping a pending client call cancels its local network task; it does not prove
that a server mutation was cancelled.

## Authority and operation IDs

The authenticated connection supplies the acting account. Handlers enqueue work
onto the simulation thread and check the expected world before executing it.
Network tasks serialize owned results after the simulation operation completes.
Each server loop processes up to four queued operations per connection,
independently of simulation stepping.

Mutation arguments include `Operation { world, id }`. Reusing an ID with the same
method and arguments returns its saved result; reusing it with different arguments
fails. Operation results and game state are saved in the same world checkpoint.
Checkpoint durability follows the existing world persistence policy.

Calls on different streams may execute in either order. Await a mutation before
making a query that depends on its effect.

## Main events, inputs, and files

`subscribe_events()` creates an independent subscription to one shared main
stream reader. It starts with the current session descriptor, then receives future
descriptors and frames in order. Frames are shared using `Arc`. A bounded queue
of 64 events reports lag explicitly. Consumers must stop using incomplete event
history after lag; the graphical client reports that reconnection is required.

`send_inputs(world, &actions)` waits for queue capacity before copying a batch.
`try_send_inputs(world, actions)` returns unsent actions when the queue is busy.
Clones share input sequencing. Queued batches retain their expected world and
cannot be retargeted onto a replacement world.

`fetch_asset(hash)` and `upload_blueprint(bytes)` use the same client and their
dedicated binary streams. Downloads verify their content hash. Uploads wait for
the server's acknowledgement before returning.

`close()` closes the shared connection and pending calls. Dropping the final
client handle also closes it. Dropping an event subscription leaves the client
and other subscriptions usable.

## Screen loading

Visible screens refresh on query changes, after successful mutations, and once
per second. Each query has at most one pending refresh. Changing its world,
session generation, filters, or page drops pending work and discards stale data.
Permission failures replace previously displayed privileged data with an error.

The main frame carries continuous simulation observations, chat, and input
acknowledgements. Society, Assets, Wallet, Market, and Industry queries use
RPC. Input acknowledgements contain a command ID and optional error text.

## Directory and scoped lists

The directory loads complete lists through `list_blocs(world)`,
`list_polities(world)`, `list_organizations(world, polity)`, and
`list_players(world, organization)`. For players, `None` selects unaffiliated
players. These methods have no cursor or page limit. An unknown parent produces
an error; an existing parent with no children returns an empty vector.

`search_identities(world, search)` searches all identity names case-insensitively
and returns matching principals plus their ancestor records. Search results and
targeted `resolve_identities` replies do not establish complete membership.
The client caches complete lists by parent, loads branches as they open, and
keeps expansion independent of selection. Search uses separate expansion state.

`list_wallets`, `gas_balances`, `list_access_profiles`, and
`storage_stock(owner, station)` also return complete authorized vectors.
Pagination is used for potentially large flat collections, including ships,
facilities, orders, stock locations, and histories. Authorization is rechecked
for each query.

## Inventory queries

`list_facilities`, `facility`, `hangar`, and `industry_catalogue` read their
domain records directly. The client tracks catalogue, directory, hangar, and
each requested inventory independently; multiple panes share an inventory
request. A failed inventory request clears that inventory's private data and
does not discard successful results for other inventories.

Query parameters and composed screen views live in the client. Request context
(world, session generation, and query) rejects superseded replies. Inventory
queries have no subscription revision, publication budget, or omitted-inventory
list. Hangar paging compares the requested cursor with the loaded query.

## Public industry

Industry RPCs enqueue typed requests with oneshot replies. Each simulation tick
processes FIFO queues in this order: service policy changes, cancellations,
cargo changes, then new work. Ordering is guaranteed within a queue; phase
ordering takes precedence across queues. Each consumer validates and applies a
whole request before replying. Repeated operation IDs replay their stored
result, and conflicting arguments or stale world IDs are rejected.

`IndustrialFacility` owns its modules, lane assignments, jobs, service policy,
revenue, and mine state. Scheduled systems advance production after dock
servicing, deliver completed output, and load mined cargo. Inventory, device
condition, electrical storage, and heat remain shared vessel state. Industry
queries run after the tick's observations and derive their DTOs from current
components. Saves contain durable facility records; restoration rebuilds
modules and compiled construction designs.

`list_public_facilities` lists published operators and rates. A customer uses
`quote_industry_job` for a recipe or blueprint, then passes the returned quote to
`order_industry_job`. The server checks the quote again and reserves the
customer's funds and station inputs together. No private Industry grant is
required to buy a published service. Ship construction also requires a delivery
aperture that permits the output owner.

The first matching customer tier selects list price, a discount, free work or
refusal. Energy, time and percentage charges use integer arithmetic and round up.
Accepted work keeps its price when an operator publishes a new policy revision.
Payment transfers once when work starts, including turnover tax. Cancelling an
unstarted job releases funds and restores its inputs. Finished goods enter the
customer's station storage; ships enter the operator's hangar under the
customer's ownership.

Reserved UEC remains subject to daily demurrage. If a reservation becomes
unfunded, the job waits for payment and can be cancelled and ordered again.
`service_jobs` exposes customer jobs and operator queues without granting access
to another customer's station inventory.

## Diplomacy

`declaration_history` returns revisions newest first, with a `before` cursor.
Ordinary diplomacy queries contain current declarations and omit the history.
`resolve_standing` returns the authenticated account's effective standing and
its source, including inherited declarations and defence agreements.

Agreements carry typed docking, basing, wanted-list, mutual-defence and tariff
terms. Active docking and basing terms grant their corresponding port access;
mutual defence affects standings. Actions by bloc officers remain ordinary
authenticated client actions. Tariffs apply to transfer receipts after turnover
tax; when multiple applicable agreements specify tariffs, the highest rate
applies once. Tax remittance does not recursively trigger another tax.

The protocol includes these records and pending bloc withdrawals. Commodity offer comparisons use a bounded query across station
books. Rebuild all bundled
firmware and test fixtures with `bash tools/build_ship_firmware.sh` after changing
the game version.
