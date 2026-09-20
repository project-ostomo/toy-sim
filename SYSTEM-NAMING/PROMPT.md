# Task: name the wormhole network

You are crawling the inhabited wormhole network and naming its systems,
starting from the system closest to Sol and working outward as a
depth-first traversal. The deliverables are `SYSTEMS-LORE.md` (discovery
and naming lore, one entry per system) and `SYSTEM-NAMES.txt` (the visited
record). Work strictly sequentially — one system at a time, no subagents —
so the crawl can be paused and resumed at any time. All state lives in
this folder; nothing depends on conversation memory.

## Workflow

1. `python3 crawl.py` — pops the next system from `queue.txt`, pushes its
   unvisited gate neighbours onto the stack, and prints a dossier: name,
   catalogue ID, sovereignty, distance from Sol, the generated star(s),
   every planet (kind, orbit, mass, temperature, atmosphere, ocean
   fraction, moons, tidal lock, biosphere), and the gate links. The map
   dump is regenerated automatically if missing.
2. Decide: famous name to keep, invented name, or catalogue station
   (rules below).
3. Append the entry to `SYSTEMS-LORE.md` (formats below).
4. `python3 crawl.py name <index> '<Name>' '<date>'` — records the name
   (also acts as the visited set; duplicates are warned).
5. Repeat until interrupted. Resume any time by running `crawl.py` again.

`crawl.py init` resets the queue to the closest system to Sol. Do not run
it unless you mean to restart the crawl.

## Canon (use it, do not contradict it)

- One shared calendar. Present day: 2426.
- **USE** — Union State of Earth. Sol, the core, the old trunk. Gate
  expansion began with practical wormholes in 2114; **Unifleet** built the
  tree-like trunk network by delivering mouth seeds at sublight speeds.
- **LFS** — League of Free Systems. A defense and arbitration association
  of sovereign polities, descended from outer colonies. Institutions
  emerge 2210–2257.
- **Great Partition, 2257**: the USE destroyed the boundary gates,
  stranding the outer colonies (a "transport siege"). **2272**: the USE
  reclamation attempt met a mature LFS slipdrive program (ship-scale FTL
  through primordial throats). Slip Wars follow. **Treaty of Concord,
  2401**: postwar settlement, demilitarized zone, re-opened trade.
- Wormholes are paired manufactured mouths. Gates anchor in high orbits
  with exclusion volumes. Link kinds on the dossier: **Trunk** (old
  Unifleet sublight-seed routes), **Regional** (sovereignty's own
  expansion), **Mesh** (post-slip LFS crosslinks), **Concord** (postwar
  inter-bloc links).
- The ten authored systems (Sol, Helion, Aurora, Meridian, Cinder, Vesper,
  Lyra, Elysium, Havoc, Terminus systems) keep their names. They are
  pre-placed and may appear as gate endpoints.

## Naming rules

1. **Keep famous real names.** Sirius, Proxima Centauri, Barnard's Star,
   Fomalhaut, Merak, ε Indi, Guniibuu and similar household or
   IAU-proper names stay. Catalogue designations (HIP, GJ, Gaia EDR3,
   Bayer letters with numbers) do not count as famous on their own.
2. **Invent names for notable systems.** A system is notable if it has a
   genuinely habitable world (breathable atmosphere, sane temperature —
   the dossier prints a HABITABLE hint), a distinctive economy or
   character justified by its worlds, a famous name, or a major hub
   role. The invented name must fit the naming institution of its
   sovereignty (below), and the full entry records the naming story.
3. **Non-notable systems stay catalogue stations.** No habitable world,
   no distinctive hook, nothing memorable: keep the catalogue ID, add a
   short entry (2–4 sentences) marked `*(catalogue station)**. These
   are the network's numbered stops — the register is *supposed* to be
   mostly numbers. Write the stub with one specific, true detail anyway.
4. **No duplicate names.** `crawl.py name` warns; never ignore it.
5. **Populations are yours to invent.** The simulation has no population
   figures. Invent plausible numbers from the system's history, worlds
   and economy — a young mining depot has thousands; an old core garden
   world has billions. Keep them internally consistent as the crawl
   proceeds (a system's neighbours should not wildly out-scale it
   without a story reason).

### Naming institutions by sovereignty

- **USE** — the Union Naming Board ratifies crew names, charters and
  jokes; bottom-up, self-deprecating, occasionally overridden by public
  campaigns. Settlement dates on the trunk: roughly 2121 (Proxima) to
  2257, spreading outward with the seed pushes.
- **Helion Commonwealth (LFS)** — Assembly of Settlements dedications:
  virtues, guild charters, founding cooperatives. Most systems were
  settled in the trunk era (c. 2160–2256) and lived through the
  Partition; mesh links are post-2272.
- **Vesper Freeports (LFS)** — names are auctioned. Commercial, honest
  about it; port names, trade words, corporate bids.
- **St Raphael Commonwealth (LFS)** — Catholic communitarian; names are
  dedicated by founding chapters: saints, orders, liturgical terms.
- **Meridian League (LFS)** — a polity of professions; names by
  committee, certified for function: Latin administrative words, guild
  terms, dry wit.
- **Lyra Research Compact (LFS)** — research polity; establish its
  conventions when first encountered.
- **Nova Partenia** — papal neutral state (Elysium system is its
  anchor); **Concord Free State**, **Terminus Protectorate** —
  independents; establish conventions when first encountered.

## Entry formats

Full entry (notable):

```
## <Name>

- **Settled:** <day month year>   (or **Gate opened:** on the USE trunk)
- **Sovereignty:** <state> (League of Free Systems / USE / independent)
- **Catalogue:** <gaia id> (<catalogue designation>), <star class>, <dist> ly
- **Population:** <invented figure>
- **System:** <the actual planets from the dossier — the settled world
  named, the giants with their moons, the oddities>
- **Links:** <every gate with its kind label and the neighbour's name>
- **Registry name:** <how the name came to be — the naming institution,
  the argument, the auction, the dedication>

<2–4 paragraphs of lore. Ground everything in the real orrery: if the
dossier says the only world is a boiling ocean, the lore lives with that.
Discovery/settlement history consistent with the canon dates. One thing
the system is genuinely best at. One living detail — a custom, a
building, a grievance. Vary length: not every system deserves four
paragraphs, and rhythm variation is part of the quality.>
```

Catalogue station (non-notable):

```
## <catalogue ID> *(catalogue station)*

- **Settled:** <year>; never promoted.
- <star class, distance> — <the architecture in one sentence>. <The
  residents and their work>. <One specific, memorable detail.>
```

## Chronology discipline

Discovery/settlement dates must trace a coherent history: the DFS order
is a rough proxy for the order regions were opened, but keep canon
hard dates fixed (2114 wormholes, 2257 Partition, 2272 reclamation, 2401
Concord, 2426 present). Trunk systems pre-2257; LFS systems were Union
outer colonies before the Partition and League members after; systems
first reached after 2272 were slip-delivered. The date column in
`SYSTEM-NAMES.txt` is the recovery point after a context reset — read its
tail before writing a new entry.
