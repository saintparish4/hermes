//! Seed list: 62 Base mainnet addresses, every one probed on-chain before being
//! written here (no address in this file is guessed).
//!
//! The mix is deliberate. It is not just proxies:
//!
//! * **OP Stack predeploys** — the `0x42…` range. Almost all are ERC-1967 proxies sharing a
//!   single admin, `ProxyAdmin` at `0x42…0018`, which makes them the clearest possible
//!   demonstration of the thesis: one authority, many contracts. `ProxyAdmin` is
//!   also its *own* admin, so it is a ready-made cycle for the resolver to survive.
//! * **Live proxies** sampled from recent Base blocks, covering transparent, UUPS and beacon.
//! * **Deliberate non-proxies** — USDC, EURC and WETH9 are in this list precisely because
//!   Hermes cannot classify them as upgradeable. USDC and EURC use the pre-1967
//!   `org.zeppelinos` slot; WETH9 is not a proxy at all. They keep the coverage number in
//!   `GET /coverage` honest instead of flattering.
//!
//! Curated seed sources replace this file once discovery is worth automating. Until then I
//! extend it by hand — `scripts/discover_seed.py` regenerates candidates.

/// One seeded address. `label` is hand-written and advisory; it is never derived on-chain.
pub struct SeedEntry {
    pub address: &'static str,
    pub label: Option<&'static str>,
}

pub static SEED: &[SeedEntry] = &[
    SeedEntry {
        address: "0x4200000000000000000000000000000000000000",
        label: Some("LegacyMessagePasser"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000002",
        label: Some("DeployerWhitelist"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000006",
        label: Some("WETH9"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000007",
        label: Some("L2CrossDomainMessenger"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x420000000000000000000000000000000000000F",
        label: Some("GasPriceOracle"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000010",
        label: Some("L2StandardBridge"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000011",
        label: Some("SequencerFeeVault"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000012",
        label: Some("OptimismMintableERC20Factory"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000013",
        label: Some("L1BlockNumber"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000014",
        label: Some("L2ERC721Bridge"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000015",
        label: Some("L1Block"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000016",
        label: Some("L2ToL1MessagePasser"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000017",
        label: Some("OptimismMintableERC721Factory"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000018",
        label: Some("ProxyAdmin"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000019",
        label: Some("BaseFeeVault"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x420000000000000000000000000000000000001A",
        label: Some("L1FeeVault"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000020",
        label: Some("EAS SchemaRegistry"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000021",
        label: Some("EAS"),
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000001",
        label: None,
    }, // op-stack predeploy
    SeedEntry {
        address: "0x4200000000000000000000000000000000000003",
        label: None,
    }, // op-stack predeploy
    SeedEntry {
        address: "0x420000000000000000000000000000000000001b",
        label: None,
    }, // op-stack predeploy
    SeedEntry {
        address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        label: Some("USDC"),
    }, // well-known
    SeedEntry {
        address: "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42",
        label: Some("EURC"),
    }, // well-known
    SeedEntry {
        address: "0xd9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA",
        label: Some("USDbC"),
    }, // well-known
    SeedEntry {
        address: "0xB79DD08EA68A908A97220C76d19A6aA9cBDE4376",
        label: Some("USD+"),
    }, // well-known
    SeedEntry {
        address: "0x0000000071727De22E5E9d8BAf0edAc6f37da032",
        label: Some("ERC-4337 EntryPoint v0.7"),
    }, // well-known
    SeedEntry {
        address: "0xE1191102BDCeA1928A93b4d6eA7Bf5C4e9207210",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xf524C1Bc1C64A2C99bc7eccf19EDe9a1d89d5a7C",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x402E0d314fD6F55348Df7CC478bAb811826e3e91",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x4B963fB4A26f082D94f964FA3c2764821Cc06Bd4",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x3b84Be4d48888a6bc385EEa93e522246B214069E",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xED57BacDc2a990B631F8817853935791C122c356",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x4955d3c5C755F654cd27ada9F085Ded00469fBc8",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x977667ac285b71da0CC4dc32f590d272d44fD6ef",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xFB384a73e9B89b01749C1127cDe4FeA20fb9F06a",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xd8Ba9D1a99Fc21f0ECA24e9b85737c28A194a4E2",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xf397910F005151b09644228573a4353818D3755d",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x76923cDDE21928ddbeC4B8BFDC8143BB6d0841a8",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xC0269FC72c0138a3A551cCf07f0819adABAa8973",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0xB078335F52F3C85b57609eBdD43C359C2c42d872",
        label: None,
    }, // live transparent proxy
    SeedEntry {
        address: "0x0770d2124C0a581C28Cfc47a659817145e6Cc137",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x61040E143A77F165Ba44543AF4A079F2C809D14b",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xCF7361fB6ACCa5FA71cB58f9c3EC7091EA8472f4",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xc3d963E0856A2c2d6F75C83C1355f680fd8F9f10",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xbC9327bC5c82f688bC1dFEBb871c8c1598E062C5",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x0000eFC4ec03a7c47D3a38A9Be7Ff1d52dD01b99",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xcB1c06554772BC855D81a6be648cC599710e1b99",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x6d27486790ce5918f1bc68bE3fCcC25304D09D31",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x931F9d2CE13212F33cEE3512768224B70820ea96",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xFFC8519CAd3a02DB4252DFcfC81F15A2BEFbb9E4",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x7eC73D41Cc5f5d6e532EE094a0cc14F1b05383D9",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0x57492dEEaD205793140C82E71d6aD39a3bEa435f",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xA238Dd80C259a72e81d7e4664a9801593F98d1c5",
        label: None,
    }, // live uups proxy
    SeedEntry {
        address: "0xCd76E4e8D7F498A728cfAfe088fB3e6fCBbfaB21",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0x5E7200a139e862C703878D89a49F810cfF8AECfA",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0x7957E5A88f0389997B352315e70aB20d77Ec410a",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0xE3772989930533e00505e1268A48a2bd35fA6480",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0xD53e31924d0CFDa200769bA80ae6b383dFec92BB",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0x40451640D2de83e33d315c2d3169dcE26Bf3F647",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0x227D920e20eBAc8A40E7D6431B7d724Bb64D7245",
        label: None,
    }, // live beacon proxy
    SeedEntry {
        address: "0xB30C6Cbc515517A1B6096AA2E2aED77862590261",
        label: None,
    }, // live beacon proxy
];
