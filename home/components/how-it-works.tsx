const steps = [
  {
    title: "Probe the slots",
    body: "ERC-1967 and EIP-1822 keep the implementation, admin and beacon at fixed storage slots. A few reads, pinned to a finalized block, classify a proxy as transparent, UUPS, beacon or admin-only.",
    tone: "bg-sky text-sky-dark",
  },
  {
    title: "Walk the authority",
    body: "Whatever can upgrade it is followed through ProxyAdmins, Safes, timelocks, roles and smart accounts, including contracts on Ethereum that act on Base through their L1→L2 alias.",
    tone: "bg-lime text-lime-dark",
  },
  {
    title: "Count the keys",
    body: "The fewest distinct keys an upgrade takes. An m-of-n Safe costs its m cheapest owners, and a key that signs under two branches is counted once. If any part is unread, there is no count.",
    tone: "bg-orchid text-orchid-dark",
  },
];

export function HowItWorks() {
  return (
    <section id="how-it-works" className="py-16 md:py-24">
      <div className="page-wrap">
        <div className="flex flex-col items-center text-center">
          <p className="t-label text-slate">How Hermes works</p>
          <h2 className="t-heading-xl mt-5">
            From slot
            <br />
            to key
          </h2>
          <p className="t-body-lg mt-7 max-w-[42rem] text-slate">
            Hermes does not analyze bytecode. It reads who can replace it: the
            upgrade entry of each proxy, and whatever stands behind that.
          </p>
        </div>
        <ol className="mt-12 grid gap-4 md:mt-16 lg:grid-cols-3">
          {steps.map((step, index) => (
            <li
              key={step.title}
              className={`panel flex min-h-[340px] flex-col justify-between p-8 md:min-h-[440px] md:p-10 ${step.tone}`}
            >
              <p className="t-heading-xl opacity-90">
                {String(index + 1).padStart(2, "0")}
              </p>
              <div>
                <h3 className="t-heading">{step.title}</h3>
                <p className="t-body mt-4 max-w-[34rem]">{step.body}</p>
              </div>
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}
