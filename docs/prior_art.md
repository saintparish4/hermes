# Prior art

Before committing to this, I wanted to know whether it already exists. Searched 2026-08-31.
**Nothing disqualifying.**

What would have stopped me: a live, maintained, public leaderboard ranking *Base application
contracts* by *upgrade authority*. I did not find one. Three things come close enough that I
expect to be asked about each, so here is what I found and where the line falls.

## L2BEAT

The closest thing that actually exists.

L2BEAT publishes upgrade-authority and permission data per chain, and its Base page states
that upgrades require both the Base Coordinator Multisig and the Base Security Council. But
its Contracts section is scoped to *infrastructure* — the documentation's own examples are
`RollupProxy`, `UpgradeExecutor`, `L1Timelock`, `Bridge`. Coverage is driven by per-project
configuration under `packages/config/src/projects`, so discovery is automated *within* a
project someone has already written a config for.

That is the distinction I care about: L2BEAT models rollup infrastructure, per project, by
configuration. Hermes models application-layer proxies on one chain, chain-wide, by
storage-slot probing. The two do not overlap on a single contract.

## DeFiScan

A fork of L2BEAT's framework aimed at DeFi rather than rollups, publishing centralization
reviews and decentralization stages per protocol at defiscan.info.

This is the nearest neighbour on the application layer and the one I looked at hardest,
because it genuinely does examine permissions and upgradeability. Where it differs: it
reviews protocols one at a time and assigns each a stage. It never inverts the index to rank
*authorities* by aggregate exposure, which is the whole point of what I am building. A
protocol-indexed review and an authority-indexed leaderboard answer different questions off
the same underlying facts.

## Dune

Base dashboards are plentiful and none of them do this. What is there is wallet rankings,
network activity, growth, and protocol performance. Nothing ranking proxies by upgrade admin,
nothing aggregating exposure per authority.

## What this does not establish

This was a web search, so it only finds what is indexed and described. It cannot prove
absence — a private tool, an unlaunched one, or one described in vocabulary I did not think
to guess would all be invisible to it. My claim is narrower than "this is novel": nothing
public and maintained currently occupies the slot I am building into.
