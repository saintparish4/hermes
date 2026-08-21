function FeatureIcon({ name }: { name: "slots" | "graph" | "price" | "rank" }) {
  const common = {
    width: 20,
    height: 20,
    viewBox: "0 0 20 20",
    fill: "none",
    "aria-hidden": true as const,
  };

  if (name === "slots") {
    return (
      <svg {...common}>
        <rect x="3" y="3" width="5.5" height="5.5" stroke="#242424" />
        <rect x="11.5" y="3" width="5.5" height="5.5" stroke="#242424" />
        <rect x="3" y="11.5" width="5.5" height="5.5" stroke="#242424" />
        <rect x="11.5" y="11.5" width="5.5" height="5.5" stroke="#242424" />
      </svg>
    );
  }

  if (name === "graph") {
    return (
      <svg {...common}>
        <circle cx="5" cy="5" r="2" stroke="#242424" />
        <circle cx="15" cy="5" r="2" stroke="#242424" />
        <circle cx="10" cy="15" r="2" stroke="#242424" />
        <path d="M6.7 6.2 8.8 13.2M13.3 6.2 11.2 13.2M7 5h6" stroke="#242424" />
      </svg>
    );
  }

  if (name === "price") {
    return (
      <svg {...common}>
        <path d="M4 15.5 8.5 9l3.2 4.2L16 5.5" stroke="#242424" strokeLinecap="round" />
        <path d="M12.5 5.5H16v3.5" stroke="#242424" strokeLinecap="round" />
      </svg>
    );
  }

  return (
    <svg {...common}>
      <path d="M4 14h12M6.5 14V8.5M10 14V5.5M13.5 14V10" stroke="#242424" strokeLinecap="round" />
    </svg>
  );
}

function FeatureCard({
  icon,
  title,
  body,
}: {
  icon: "slots" | "graph" | "price" | "rank";
  title: string;
  body: string;
}) {
  return (
    <article className="rounded-[40px] border border-ash bg-parchment p-10">
      <FeatureIcon name={icon} />
      <h3 className="t-subheading mt-6 text-off-black">{title}</h3>
      <p className="t-body mt-4 text-graphite">{body}</p>
    </article>
  );
}

function GradientArt() {
  return (
    <div className="relative h-44 w-full overflow-hidden rounded-[28px] md:h-full md:min-h-[220px]" aria-hidden>
      <div className="absolute -right-6 -top-8 size-44 rounded-full bg-coral/80 blur-[36px]" />
      <div className="absolute right-8 top-10 size-40 rounded-full bg-sky-blue/80 blur-[32px]" />
      <div className="absolute bottom-0 right-16 size-36 rounded-full bg-mint/90 blur-[28px]" />
      <div className="absolute bottom-6 right-4 size-24 rotate-12 rounded-[20px] bg-gold/70 blur-[12px]" />
      <div className="absolute left-6 top-8 size-28 -rotate-6 rounded-[24px] border border-white/40 bg-white/20" />
    </div>
  );
}

export function ConnectEverything() {
  return (
    <section id="connect" className="py-20 md:py-24">
      <div className="page-wrap">
        <p className="section-label">Connect everything, effortlessly</p>
        <div className="mt-6 grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)] lg:items-end">
          <h2 className="t-heading-lg max-w-[16ch] text-off-black">
            Every proxy, tied to the key that actually governs it.
          </h2>
          <p className="t-body max-w-xl text-graphite lg:justify-self-end">
            No new agents. No decompiler. Three storage reads classify a
            proxy; interface probing walks the admin chain until a terminal
            authority. Nested Safes, ProxyAdmins, and timelocks collapse to
            one row — the key that can move the money.
          </p>
        </div>

        <div className="mt-14 grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <article className="flex flex-col justify-between rounded-[40px] bg-periwinkle-mist p-10 md:col-span-2">
            <div className="grid gap-8 md:grid-cols-[minmax(0,1fr)_minmax(0,0.9fr)] md:items-center">
              <div>
                <p className="t-caption text-graphite">Upgrade-dominates</p>
                <h3 className="t-subheading mt-4 text-off-black">
                  If a key can replace the implementation, exposure is full
                  custody.
                </h3>
                <p className="t-body mt-4 max-w-md text-graphite">
                  Arbitrary code replacement subsumes every other capability.
                  An audited contract with a live admin key is an unaudited
                  contract plus a promise. Hermes prices that fact, not the
                  audit.
                </p>
              </div>
              <GradientArt />
            </div>
          </article>

          <FeatureCard
            icon="slots"
            title="Probe the slots"
            body="ERC-1967 and EIP-1822 hand over the authority graph for free. Classification is three RPC reads against fixed constants — Transparent, UUPS, Beacon, or not upgradeable."
          />
          <FeatureCard
            icon="graph"
            title="Walk the graph"
            body="Safe, Ownable, ProxyAdmin, Timelock, EOA. Recurse to depth four with cycle detection. Two proxies with different ProxyAdmins owned by the same Safe group under that Safe."
          />
          <FeatureCard
            icon="price"
            title="Price the blast radius"
            body="Native ETH plus a fixed token list, batched through Multicall3, priced from DeFiLlama. authority_var is the sum of custody across every proxy that terminal key controls."
          />
          <FeatureCard
            icon="rank"
            title="Publish the ranking"
            body="A public leaderboard and a JSON API. No login. Coverage percentages are computed and printed. Unknown is a result, never a guess."
          />
        </div>
      </div>
    </section>
  );
}
