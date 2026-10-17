# The oracle's contradictions, read by hand

Every row `hermes oracle` contradicted on 2026-10-17 (model 4, Base block 52,734,116), each
repeated on a second fresh fork before it stood. Read again by hand on a third anvil fork with
`cast`: the owner Hermes names calls `upgradeToAndCall` to re-install the proxy's own current
implementation, which no allowlist of implementations can refuse, and the revert is copied as the
node returned it. Nothing below was produced by Hermes.

| Proxy | Owner Hermes names (code size) | Current implementation | Re-install by that owner |
|---|---|---|---|
| `0xd2b6d5670ca24ede33b9e285036a2470f78400e0` | `0xDF606c9BE86e972a5447Cf4Cb210EB34871c742F (code 23 B)` | `0x32403455357589523435db18b414057d7ce05051` | execution reverted: NotApprovedImplementation |
| `0xc3f7ea98c83a147ace998d6de91cb5612e5ca6fc` | `0xc6a39E2f9A7626612292AD56e76d738DB43596E4 (code 23 B)` | `0xaf5a4f6f6640734d7d000321bb27de40d4ae91f6` | execution reverted, data: "0x" |
| `0xb18949a74bc59e87992b145af3a7090e5a36d121` | `0xFF9d76F5ff713e1ce4808eef18908C70C786f73C (code 0 B)` | `0x26ba1566bba5660eeccc6c052e953e945bf28550` | execution reverted: Only Mamo Strategy Registry can call |
| `0x28f61ad0b5fd02de121f659e792c7c14ca40edc7` | `0x3dAaFDb01e73B17210E11a5523c932f8A64aAC1e (code 0 B)` | `0x8b645cdfe21f9553e51231dcf87c9edbdfd7d92c` | execution reverted: UnauthorizedImplementation |
| `0xa51b0db5067924c53517481d3a27e17906800c18` | `0xf57fc7b8F823760ac6F91aed9FF150aCe5dcb566 (code 0 B)` | `0x7240519388acd7632ff38753f514fcc993abed09` | execution reverted: Only Mamo Strategy Registry can call |
| `0x1a79e213c0b590ee6abec62f5a727ad0418b7a07` | `0xff26d8b28501B6b5A74F83c378faa9771B12C21d (code 23 B)` | `0x32403455357589523435db18b414057d7ce05051` | execution reverted: NotApprovedImplementation |
| `0xaaea60474c6d6397607dd2bcdc610c307017d05b` | `0x5183E0203858aa3E3bc3A7D9cb41875a4c0A6216 (code 0 B)` | `0x02be886a3b2802177181f4734380cb1f4bac4bfb` | execution reverted, data: "0x" |
| `0x6bc88de5db691dee5ca062fe46f08f79005276cc` | `0x3c31232FDdeDFceA4dfF5eC2f68c38603C008f4e (code 61 B)` | `0x1585e6feb5da8aea107290596f747f81522123de` | execution reverted: InvalidUpgradePath(0x1585E6FEb5da8AEA107290596F747f81522123d |
| `0xe25841bf42861c07d911282502b9276c3b20d21b` | `0x634C241a6eb751255BEDd63401590d9C0f69D116 (code 61 B)` | `0x1585e6feb5da8aea107290596f747f81522123de` | execution reverted: InvalidUpgradePath(0x1585E6FEb5da8AEA107290596F747f81522123d |
| `0x516cab95d1b76f75f366e7d163b7f6259a39f125` | `0x486E9a786F697c35e1fBae2828919f382E9120a3 (code 61 B)` | `0x1585e6feb5da8aea107290596f747f81522123de` | execution reverted: InvalidUpgradePath(0x1585E6FEb5da8AEA107290596F747f81522123d |
| `0x1370b00036df3edfdd623607db43c871357b514c` | `0xaBc8857581Ad4FCFd68741d4c34d15841485EbD6 (code 61 B)` | `0x06fb7d2650c308320f6791d0543767735305fec7` | execution reverted, data: "0x" |
| `0x01d604e2471d57eece8a6866061b06f0c2b7d5d1` | `0x8153f6FfE5E8bCe080f05219cA4868c3b74e62e8 (code 61 B)` | `0x06fb7d2650c308320f6791d0543767735305fec7` | execution reverted, data: "0x" |
| `0x38d4479cb3accfbb54877b4fd3484b5727cb6bd7` | `0x2e93EBC3f93df6b4170d41D8cc700921cc2De9Aa (code 61 B)` | `0x6c8577fa9b10807f7485f6476c2afe0b8d61d1e7` | execution reverted: Only Mamo Strategy Registry can call |
| `0xc762becec06fbd4b33002f31da7e1866bd04cf8f` | `0xf880869d351d9a645F2789E9a9022FFA22320a95 (code 61 B)` | `0x810c567c80e9c08d8126f273d792adee15c45eec` | execution reverted: InvalidStrategy |

Three readings, none of which is "this key alone can replace the code":

- **The caller is refused** (`Only Mamo Strategy Registry can call`, 3): another contract is the
  gate, and the owner cannot upgrade at all.
- **The destination is a registry's to approve** (`NotApprovedImplementation`,
  `UnauthorizedImplementation`, `InvalidUpgradePath`, `InvalidStrategy`, 7): the owner can move
  only to an implementation a registry has approved as the next one, so whoever controls that
  registry takes part in every upgrade.
- **A bare revert** (4): no reason given; stable across two forks.

A 23-byte owner is an EIP-7702 delegated key; a 61-byte owner is a smart account proxy.
