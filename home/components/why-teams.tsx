const reasons = [
  {
    title: "Rank keys, not contracts",
    body: "A zero-balance controller proxy governing a high-TVL protocol ranks correctly rather than near zero. The unit of analysis is the authority.",
  },
  {
    title: "Depth is a column, not a score",
    body: "A 1-of-1 EOA on $10M and a 5-of-9 Safe on $50M are both visible. Composite risk scores hide the inputs. Hermes does not.",
  },
  {
    title: "Unknown is first-class",
    body: "If the interface does not match, the row says Unknown. Inferring an authority structure that was not identified would discredit the entire tool.",
  },
  {
    title: "Base only, on purpose",
    body: "One chain, honest coverage, a single binary. Multi-chain and alerting are how this becomes an unfinished project instead of a shipped map.",
  },
];

export function WhyTeams() {
  return (
    <section id="why" className="py-20 md:py-24">
      <div className="page-wrap">
        <p className="section-label">Why teams choose Hermes</p>
        <h2 className="t-heading-lg mt-6 max-w-[18ch] text-off-black">
          Built to answer one question, and to stop there.
        </h2>
        <div className="mt-14 grid gap-4 md:grid-cols-2">
          {reasons.map((reason, index) => (
            <article
              key={reason.title}
              className="rounded-[40px] border border-ash p-10"
            >
              <p className="t-caption text-smoke">
                {String(index + 1).padStart(2, "0")}
              </p>
              <h3 className="t-subheading mt-6 text-off-black">{reason.title}</h3>
              <p className="t-body mt-4 text-graphite">{reason.body}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
