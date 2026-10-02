const faqs = [
  {
    q: "What does Hermes measure?",
    a: "Who can change the code behind a deployed contract. For each upgradeable proxy it finds the authority at the root of the upgrade path, the fewest distinct keys an upgrade takes, and whether a timelock stands in the way. It does not put a dollar figure on any of it.",
  },
  {
    q: "Why not list contract admins the way explorers do?",
    a: "The immediate admin is often a ProxyAdmin contract that something else owns. Two proxies with different ProxyAdmins owned by the same Safe are controlled by that Safe, so Hermes groups them under it.",
  },
  {
    q: "How is the key count calculated?",
    a: "A single key is 1. A 2-of-3 Safe is 2. A Safe whose owners are themselves Safes costs the sum of its cheapest owners, and a key that signs under two branches is counted once. If any part of the chain is unknown, cut short or cyclic, there is no count.",
  },
  {
    q: "What does unknown mean?",
    a: "Either the contract matched no interface Hermes recognizes, or a read went unanswered. Both are shown with a reason. Unknown does not mean safe, and Hermes will not infer a structure it did not positively identify.",
  },
  {
    q: "Is this a bug detector?",
    a: "No. Hermes does not inspect bytecode and makes no claim about vulnerabilities. Its claims are about capability: who is able to upgrade a contract.",
  },
  {
    q: "What is covered?",
    a: "Base, with Ethereum read only to follow L1 contracts that act on Base. ERC-1967 transparent, UUPS and beacon proxies, EIP-1822, and OP Stack admin-only predeploys. Diamond, inherited and eternal storage, and the older ZeppelinOS slot are named gaps. The index is a sample of proxies on Base, not a census.",
  },
  {
    q: "Does it track changes?",
    a: "Yes, from the first time Hermes sees an address. Changes are published as JSON and as an Atom feed. There are no alerts, accounts or webhooks.",
  },
];

export function Faq() {
  return (
    <section id="faq" className="py-16 md:py-24">
      <div className="page-wrap">
        <div className="flex flex-col items-center text-center">
          <p className="t-label text-slate">FAQ</p>
          <h2 className="t-heading-xl mt-5">
            Fair
            <br />
            questions
          </h2>
        </div>
        <div className="mx-auto mt-12 flex max-w-5xl flex-col gap-3 md:mt-16">
          {faqs.map((item) => (
            <details
              key={item.q}
              className="faq-item group rounded-[24px] bg-mist px-7 open:bg-fog md:px-9"
            >
              <summary className="flex cursor-pointer items-center justify-between gap-8 py-7">
                <span className="t-title">{item.q}</span>
                <span
                  aria-hidden
                  className="flex size-10 shrink-0 items-center justify-center rounded-full bg-ink text-[20px] leading-none text-paper transition-transform duration-200 group-open:rotate-45"
                >
                  +
                </span>
              </summary>
              <p className="t-body max-w-3xl pb-8 pr-10 text-slate">{item.a}</p>
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}
