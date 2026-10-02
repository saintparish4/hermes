import { APP_URL } from "@/components/links";

const chain = [
  { role: "Proxy", name: "L2StandardBridge", detail: "0x4200…0010 on Base" },
  { role: "Admin", name: "ProxyAdmin", detail: "0x4200…0018 on Base" },
  { role: "Owner", name: "L1 alias", detail: "no code on Base" },
  { role: "Root", name: "2-of-2 Safe", detail: "0x7bB4…595c on Ethereum" },
];

function AuthorityChain() {
  return (
    <div className="panel bg-sky-light p-6 text-sky-dark md:p-12">
      <div className="flex flex-wrap items-baseline justify-between gap-4">
        <p className="t-label">One authority, followed to its keys</p>
        <p className="t-label opacity-70">Read at Base block 51,526,000</p>
      </div>
      <ol className="mt-8 grid gap-7 md:mt-10 lg:gap-x-9 lg:grid-cols-[repeat(4,minmax(0,1fr))_minmax(0,0.9fr)]">
        {chain.map((node) => (
          <li
            key={node.role}
            className="relative rounded-[24px] bg-paper p-6 after:absolute after:left-1/2 after:top-full after:z-10 after:-translate-x-1/2 after:translate-y-[-6px] after:text-[22px] after:font-bold after:content-['↓'] lg:after:left-[calc(100%+18px)] lg:after:top-1/2 lg:after:-translate-y-1/2 lg:after:content-['→']"
          >
            <p className="t-label opacity-60">{node.role}</p>
            <p className="t-title mt-8">{node.name}</p>
            <p className="mt-2 font-mono text-[13px] opacity-70">
              {node.detail}
            </p>
          </li>
        ))}
        <li className="rounded-[24px] bg-sky-dark p-6 text-sky-light">
          <p className="t-label opacity-70">Fewest keys to upgrade</p>
          <p className="t-display mt-4 !text-[88px] text-lime">11</p>
          <p className="mt-2 font-mono text-[13px] opacity-80">
            a 3-of-6 Safe and an 8-of-11 Safe
          </p>
        </li>
      </ol>
    </div>
  );
}

export function Hero() {
  return (
    <section id="top" className="pb-6 pt-8 md:pt-14">
      <div className="page-wrap">
        <div className="flex flex-col items-center text-center">
          <a
            href="#features"
            className="t-label inline-flex items-center gap-3 rounded-full bg-fog py-2 pl-2 pr-5 text-ink no-underline"
          >
            <span className="rounded-full bg-flame px-3 py-1.5 text-ink">
              New
            </span>
            History and a change feed
          </a>
          <h1 className="t-display mt-8">
            Who holds
            <br />
            the upgrade <span className="text-flame">keys?</span>
          </h1>
          <p className="t-body-lg mt-8 max-w-[44rem] text-slate">
            Hermes maps upgrade authority on Base: who can change the code
            behind a deployed contract, through every ProxyAdmin, Safe, timelock
            and role, down to the keys at the bottom.
          </p>
          <div className="mt-9 flex flex-wrap items-center justify-center gap-3">
            <a className="btn" href={APP_URL}>
              Open the map
            </a>
            <a className="btn btn-outline" href="#how-it-works">
              How it works
            </a>
          </div>
        </div>
        <div className="mt-12 md:mt-16">
          <AuthorityChain />
        </div>
      </div>
    </section>
  );
}
