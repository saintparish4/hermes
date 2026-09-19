# Hand-verified addresses

Each row is an address whose expected answer was established by reading the chain **without
Hermes**: raw `eth_getStorageAt`, `eth_getCode` and `eth_call` requests at the pinned blocks,
cross-checked against the explorer and Safe pages linked in the last column. The expectations
live in [`tests/verified.json`](../tests/verified.json), together with a longer note on how
each one was checked. The chain's answers at the same blocks live in
[`tests/fixtures/`](../tests/fixtures).

Two jobs check the table:

- **`cargo test`** (every PR). Replays each fixture through the production pipeline and
  compares the result to the table. Offline and deterministic, so red means Hermes regressed.
- **`hermes verify`** (scheduled, allowed to fail). Runs the same comparison against the live
  chain. Red means Hermes regressed *or the chain moved*: an admin was transferred, a
  threshold changed. Re-record the fixture at the new head and diff the two files to see which.

The rows are chosen by shape, not by TVL. A top-ten-by-TVL list would be ten Safes and would
miss every branch that has actually broken a resolver, including the one that produced this
table: an L1→L2 alias published as a single key.

No row covers a timelock yet, because no address in the seed has one in its admin chain.

<!-- table -->
| # | Contract | Shape | Kind | Root | Chain | Keys | Timelock | Confidence | Checked at | Evidence |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | ProxyAdmin `0x4200…0018` | Self-administered: its admin slot is itself, so the walk leaves through owner() into an L1 alias | `transparent` | safe `0x7bB4…595c` | ethereum | 11 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x4200000000000000000000000000000000000018#readContract) [2](https://app.safe.global/settings/setup?safe=eth:0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c) [3](https://app.safe.global/settings/setup?safe=eth:0x9855054731540A48b28990B63DcF4f33d8AE46A1) [4](https://app.safe.global/settings/setup?safe=eth:0x20AcF55A3DCfe07fC4cecaCFa1628F788EC8A4Dd) |
| 2 | L2StandardBridge `0x4200…0010` | The headline: a predeploy whose ProxyAdmin is owned by an L1 contract through its L2 alias | `transparent` | safe `0x7bB4…595c` | ethereum | 11 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x4200000000000000000000000000000000000010) [2](https://etherscan.io/address/0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c) |
| 3 | L1MessageSender (legacy predeploy) `0x4200…0001` | AdminOnly: admin slot set, implementation never set, still a live upgrade authority | `admin_only` | safe `0x7bB4…595c` | ethereum | 11 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x4200000000000000000000000000000000000001) |
| 4 | USDbC `0xd9aA…b6CA` | An m-of-n Safe as the direct admin (3-of-6 over EOAs) | `transparent` | safe `0xd94E…f94f` | base | 3 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xd9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA) [2](https://app.safe.global/settings/setup?safe=base:0xd94E416cf2c7167608B2515B7e4102B41efff94f) |
| 5 | Unlabelled transparent proxy `0xd8Ba…a4E2` | A ProxyAdmin owned by a Safe: the root is the Safe, not the immediate admin | `transparent` | safe `0x920F…5377` | base | 3 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x9CAAB947F1E02A192AFda15c9fCe8838a8d4755B#readContract) [2](https://app.safe.global/settings/setup?safe=base:0x920F6A03EaFc80C5C4119383AEE56CeCea115377) |
| 6 | Unlabelled transparent proxy `0xC026…8973` | A genuine single key: the admin has no code on Base and none behind its alias on Ethereum | `transparent` | eoa `0x0000…c000` | base | 1 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x000000dBc80bF780c6dc0ca16ed071b1F00Cc000) [2](https://etherscan.io/address/0xeeef00dbc80bf780c6dc0ca16ed071b1f00caeef) |
| 7 | Unlabelled transparent proxy `0x402E…3e91` | Expected Unknown: the admin has code and answers none of the probes | `transparent` | unresolved: `unrecognized_interface` | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x31e99E05fee3DCE580af777C3fD63eE1B3B40c17#code) |
| 8 | USD+ `0xB79D…4376` | UUPS confirmed, access control not recognized: proxiableUUID() says UUPS, owner() is absent | `uups` | unresolved: `unrecognized_interface` | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xB79DD08EA68A908A97220C76d19A6aA9cBDE4376#readProxyContract) [2](https://basescan.org/address/0xe1201f02C02e468c7fF6F61AFff505A859673cfD#readContract) |
| 9 | Unlabelled UUPS proxy `0xFFC8…b9E4` | UUPS resolved through owner(): confirmed UUPS, root found, capped at Medium | `uups` | eoa `0xb045…7adb` | base | 1 | 0s | medium | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xFFC8519CAd3a02DB4252DFcfC81F15A2BEFbb9E4#readProxyContract) [2](https://basescan.org/address/0xb045571F321dfF9DE46eCc204D128aa68BE47adb) |
| 10 | Unlabelled implementation-slot proxy `0xA238…d1c5` | Implementation slot and no admin, but the implementation denies being UUPS | `uups` | unresolved: `uups_unconfirmed` | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xA238Dd80C259a72e81d7e4664a9801593F98d1c5#code) [2](https://basescan.org/address/0xA4AbC5FcBA6D0d7E3D144d6dbF6cb6128599dFdB#code) |
| 11 | Unlabelled beacon proxy `0xCd76…aB21` | Beacon: the root is whoever controls the beacon, shared here by five proxies | `beacon` | safe `0xc509…39Aa` | base | 2 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x000000005aE5f42270cDf0E41960840e7FB1b2dc#readContract) [2](https://app.safe.global/settings/setup?safe=base:0xc50932edd1c14272aa35324dfc45a19ec57839aa) |
| 12 | USDC `0x8335…2913` | Not covered: the pre-1967 ZeppelinOS slot, kept to keep coverage honest | `zeppelin_os` | unresolved | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913#readProxyContract) |
| 13 | WETH9 `0x4200…0006` | Not upgradeable: code present, every slot empty | `not_upgradeable` | unresolved | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x4200000000000000000000000000000000000006#code) |
<!-- /table -->

The evidence links point at explorer pages as they are *now*; the "Checked at" blocks are
what the expectation and the fixture were pinned to.
