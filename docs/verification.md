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
| 8 | USD+ `0xB79D…4376` | UUPS confirmed, gated by OpenZeppelin AccessControl rather than an owner, and the role's holders are not listed on chain | `uups` | role_gated `0xB79D…4376` | base | unknown | 0s | medium | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xB79DD08EA68A908A97220C76d19A6aA9cBDE4376#readProxyContract) [2](https://basescan.org/address/0xe1201f02C02e468c7fF6F61AFff505A859673cfD#readContract) |
| 9 | Unlabelled UUPS proxy `0xFFC8…b9E4` | UUPS resolved through owner(): confirmed UUPS, root found, capped at Medium | `uups` | eoa `0xb045…7adb` | base | 1 | 0s | medium | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xFFC8519CAd3a02DB4252DFcfC81F15A2BEFbb9E4#readProxyContract) [2](https://basescan.org/address/0xb045571F321dfF9DE46eCc204D128aa68BE47adb) |
| 10 | Unlabelled implementation-slot proxy `0xA238…d1c5` | Implementation slot and no admin, but the implementation denies being UUPS | `uups` | unresolved: `uups_unconfirmed` | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0xA238Dd80C259a72e81d7e4664a9801593F98d1c5#code) [2](https://basescan.org/address/0xA4AbC5FcBA6D0d7E3D144d6dbF6cb6128599dFdB#code) |
| 11 | Unlabelled beacon proxy `0xCd76…aB21` | Beacon: the root is whoever controls the beacon, shared here by five proxies | `beacon` | safe `0xc509…39Aa` | base | 2 | 0s | high | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x000000005aE5f42270cDf0E41960840e7FB1b2dc#readContract) [2](https://app.safe.global/settings/setup?safe=base:0xc50932edd1c14272aa35324dfc45a19ec57839aa) |
| 12 | USDC `0x8335…2913` | Not covered: the pre-1967 ZeppelinOS slot, kept to keep coverage honest | `zeppelin_os` | unresolved | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913#readProxyContract) |
| 13 | WETH9 `0x4200…0006` | Not upgradeable: code present, every slot empty | `not_upgradeable` | unresolved | — | — | — | — | Base 51526000 / L1 26013200 | [1](https://basescan.org/address/0x4200000000000000000000000000000000000006#code) |
| 14 | Beacon proxy, before `0x9Caa…b674` | The largest root on 2026-09-20: one key behind a shared beacon, the block before it handed the beacon to a Safe | `beacon` | eoa `0x21eb…B5fc` | base | 1 | 0s | high | Base 51927177 / L1 26079703 (history: true only then) | [1](https://basescan.org/address/0x9Caa0e7277ce86A4644F2D10b72561080531b674) [2](https://basescan.org/address/0xe68ED13998fd48497EAA3b52e20823605D8d7706#readContract) |
| 15 | Beacon proxy, after `0x9Caa…b674` | The same proxy one block later: a root that changed hands, which only history can show | `beacon` | safe `0x6454…Bf15` | base | 2 | 0s | high | Base 51927178 / L1 26079703 (history: true only then) | [1](https://basescan.org/address/0x9Caa0e7277ce86A4644F2D10b72561080531b674) [2](https://basescan.org/address/0xe68ED13998fd48497EAA3b52e20823605D8d7706#readContract) [3](https://app.safe.global/settings/setup?safe=base:0x6454cf0127a153295435160768C85225Cd19Bf15) |
| 16 | EIP-7702 account `0x0F7c…AB38` | An externally owned account that delegated its code (EIP-7702) and so reads as a UUPS proxy: its own key is the authority | `uups` | eoa `0x0F7c…AB38` | base | 1 | 0s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x0F7cDA47d426B4f8DDdD4183829A938CFBDAAB38) |
| 17 | ProxyAdmin under an EIP-7702 key `0x0346…7718` | A ProxyAdmin whose owner is an EIP-7702 delegated EOA, one of two such keys behind 25 indexed proxies each | `transparent` | eoa `0x7FA6…D27C` | base | 1 | 0s | high | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x034651d50c3Caf5cF91d646ad1a8092738cc7718) [2](https://basescan.org/address/0x7cfd2c84f2313f8cf46f69f0471344d469613b85#readContract) [3](https://basescan.org/address/0x7fa6d7d62392f7464f87518de13920f85b99d27c) |
| 18 | UUPS proxy owned by a smart account `0x38d4…6bd7` | A UUPS proxy whose owner() is a MultiOwnable smart account (Coinbase Smart Wallet): any one of its signers upgrades it | `uups` | smart_account `0x2e93…e9Aa` | base | 1 | 0s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x38d4479cB3AccFBb54877B4FD3484B5727cB6bd7) [2](https://basescan.org/address/0x2e93ebc3f93df6b4170d41d8cc700921cc2de9aa) [3](https://github.com/coinbase/smart-wallet/blob/main/src/MultiOwnable.sol) |
| 19 | Smart account, scheme unread `0x1366…B619` | A UUPS proxy that is itself an ERC-4337 account of a scheme Hermes does not read: a real root with no key count | `uups` | smart_account `0x1366…B619` | base | unknown | 0s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x136607334625f802AC2Ed58d3e3F87682a21B619) |
| 20 | AccessControl, upgrade role empty `0x66d6…6Af0` | An AccessControlEnumerable contract whose UPGRADER_ROLE has no holder, but whose admin role does: that holder can grant itself the upgrade | `uups` | role_gated `0x66d6…6Af0` | base | 1 | 0s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x66d63089795C6697c805ffa6ade58484A4426Af0#readProxyContract) |
| 21 | UUPS proxy, owner renounced `0xF1CC…1D41` | A UUPS proxy whose owner() is 0xff..ff: a root no one holds a key for, not one key | `uups` | sentinel `0xFFfF…FFfF` | base | unknown | 0s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0xF1CC2116366131409f04eB25d8619Eca73171D41#readProxyContract) |
| 22 | ProxyAdmin under a 72h timelock `0x2910…9118` | A ProxyAdmin owned by a TimelockController: the delay is read, the proposer and executor roles are not listed on chain | `transparent` | timelock `0xf425…155a` | base | unknown | 259200s | medium | Base 52003751 / L1 26092419 | [1](https://basescan.org/address/0x291088312150482826b3A37d5A69a4c54DAa9118) [2](https://basescan.org/address/0xf425ed48483B49cF10C8a7f6cFd25dFD86d3155a#readContract) |
<!-- /table -->

The evidence links point at explorer pages as they are *now*; the "Checked at" blocks are
what the expectation and the fixture were pinned to.
