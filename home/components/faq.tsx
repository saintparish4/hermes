const faqs = [
  {
    q: "What does Hermes actually measure?",
    a: "If this authority is compromised tomorrow, how much money moves, and how long do you have to react. Hermes indexes upgradeable proxies on Base, resolves each admin to a terminal governance structure, and ranks those authorities by dollar exposure — not by findings in the implementation.",
  },
  {
    q: "Why not list contract admins the way explorers already do?",
    a: "Explorers list contracts and their admins. Hermes inverts the index and ranks the admins by what they control. Two proxies with different ProxyAdmin contracts owned by the same Safe must group under that Safe. Grouping on the immediate admin fragments the picture and understates exposure.",
  },
  {
    q: "How is compromise depth calculated?",
    a: "It is the minimum distinct key compromises required to execute an upgrade. An EOA admin is 1. A 2-of-3 Safe is 2. Nested structures multiply out, recursively, to depth four. A TimelockController is modeled as a single node carrying a delay — proposer and executor roles are not split in v1.",
  },
  {
    q: "What does Unknown mean on the leaderboard?",
    a: "The admin address did not match a known interface: Safe, Ownable, ProxyAdmin, Timelock, or EOA. Unknown is a legitimate terminal output and is surfaced as such. Hermes will not infer a structure it did not positively identify.",
  },
  {
    q: "Is this a bug detector?",
    a: "No. Slither, Mythril, and Foundry occupy that space. Hermes does not inspect bytecode, and will not grow in that direction. The 2026 loss data shows the money leaves through governance, not through reentrancy.",
  },
  {
    q: "Which chains and proxy patterns are covered?",
    a: "Base only. v1 covers ERC-1967 Transparent, UUPS, and Beacon proxies, plus EIP-1822. Diamond / EIP-2535, inherited storage, and eternal storage are named gaps, not silent misses. Coverage percentages are computed from the scan, not estimated.",
  },
];

export function Faq() {
  return (
    <section id="faq" className="py-20 md:py-24">
      <div className="page-wrap">
        <p className="section-label">FAQ</p>
        <h2 className="t-heading-lg mt-6 text-off-black">Questions, answered.</h2>
        <div className="mt-10 border-t border-ash">
          {faqs.map((item) => (
            <details key={item.q} className="faq-item group border-b border-ash">
              <summary className="flex cursor-pointer items-center justify-between gap-8 py-10">
                <span className="t-subheading text-off-black">{item.q}</span>
                <span
                  aria-hidden
                  className="shrink-0 text-[20px] leading-none text-off-black transition-transform duration-200 group-open:rotate-180"
                >
                  ↓
                </span>
              </summary>
              <p className="t-body max-w-3xl pb-10 pr-12 text-graphite">{item.a}</p>
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}
