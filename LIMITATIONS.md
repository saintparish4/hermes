# Limitations

What Hermes does not see, and what its numbers do not mean. Each item is a gap in coverage, not
a finding about any contract.

## Coverage

- **The index is a sample.** Discovery reads 1,000-block windows every 500,000 blocks and admits
  at most three addresses per implementation, beacon or admin. "An authority reaches N proxies"
  counts indexed proxies only. For a clone family it is at most three however many clones exist.
- **Proxy patterns.** ERC-1967 (transparent, UUPS, beacon), EIP-1822 and OP Stack admin-only
  predeploys are covered. Inherited storage, eternal storage, Diamond (EIP-2535) and the
  ZeppelinOS slot are not. USDC and EURC on Base use the ZeppelinOS slot and are reported as not
  covered on purpose.
- **Non-proxy contracts** under the same authority are invisible. A vault that holds funds and
  answers to the same Safe does not appear in that Safe's reach.
- **Upgrade privileges only.** Pause, mint, freeze, oracle and fee roles are not read.

## Resolution

- **Interfaces are duck-typed.** A contract is a Safe because it answers `getOwners()` and
  `getThreshold()`. A contract that answers them without behaving like a Safe is misread; the
  precedence among interfaces is fixed and documented, and an ambiguous contract loses confidence.
- **UUPS authority is inferred.** For a UUPS proxy, `owner()`, an `AccessControl` role or an
  account's signers are taken to be what `_authorizeUpgrade` checks. That is a convention, which
  is why every UUPS root is at most Medium.
- **Timelocks are one node.** The delay is read; the proposer and executor role holders are not,
  because `TimelockController` does not enumerate them. A timelock root has no key count
  (`roles_unread`).
- **`AccessControl` members** are read only when the contract enumerates them. A plain
  `AccessControl` keeps no list, so its root has no key count (`roles_unread`). The holders of the
  upgrade role's admin role are counted because they can grant themselves the role; a longer chain
  of admin roles is not followed.
- **Smart accounts.** ERC-4337 accounts are recognized by `entryPoint()`. Only MultiOwnable
  accounts (Coinbase Smart Wallet) have their signers read. Others are roots with
  `account_keys_unread`. A passkey signer counts as one key with no address.
- **Keys are counted, not people.** Two keys held by one person count as two. A key that signs
  in several places under one root is counted once: when it does, the fewest distinct keys are
  searched for, and when that search would be too large the count is unknown
  (`shared_signers`) rather than an overstatement.
- **An unknown owner poisons a Safe's count**, even when the known owners alone would meet the
  threshold with single keys. This is deliberately conservative.
- **Depth is capped at four links.** A deeper chain is reported as truncated, with no key count.
- **EIP-7702.** A delegated EOA is a key. Whether anyone still holds that key is not something the
  chain says, which is true of every EOA.
- **Sentinels** are a fixed list: `0x…dEaD`, `0xff…ff`, the precompiles `0x01`–`0x0a` and
  `0x100`. Other burn addresses read as keys.

## History

- **History starts at first sight.** Hermes records changes between its own scans. Anything that
  happened before an address was first scanned is not in the store, and "first seen" means first
  seen by Hermes.
- **Changes are sampled at scan time.** Two changes between scans can read as one, or as none if
  a value changed and changed back. Bisection finds a boundary, not every transition.
- **Only single-read changes are pinned to a block.** A changed root or key count keeps the two
  scan blocks that bracket it.
- **Full-history backfill needs a keyed endpoint.** The public Base endpoint accepts `eth_getLogs`
  over about 2,000 blocks at a time, even for one address (measured 2026-09-30), so reading a
  contract's whole event history costs tens of thousands of calls.

## Reading the chain

- **Public endpoints misbehave.** Under load the public Base endpoint returns empty code and zero
  storage for contracts that have both. Empty reads are confirmed by a second read before they
  become verdicts, and a node that will not answer produces "undetermined", never "no".
- **Multicall3 is not used for probes.** One contract that burns all gas in a batched call starves
  the calls after it, which would turn real answers into false "no"s. Probes are sequential.

## Claims Hermes does not make

- That any key is compromised, or that anyone will act.
- That a contract is safe or unsafe. There is no score.
- Anything in dollars. Exposure is not computed.
- That it covers every proxy on Base.
