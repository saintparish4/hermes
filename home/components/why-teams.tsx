const reasons = [
  {
    title: "Authorities, not contracts",
    body: "Explorers list a contract and its admin. Hermes inverts that and lists the authority with everything it can upgrade.",
  },
  {
    title: "A count, not a score",
    body: "A single key and a 5-of-9 Safe are both shown as what they are. A composite score would hide its inputs, so there is none.",
  },
  {
    title: "Unknown is an answer",
    body: "If a contract matches no interface Hermes recognizes, the row says so. An unanswered read is never turned into “no”.",
  },
  {
    title: "Capability, never intent",
    body: "Hermes says who can upgrade a contract. It does not say anyone will, and it is not a bug detector.",
  },
];

export function WhyTeams() {
  return (
    <section id="why" className="py-4 md:py-8">
      <div className="page-wrap">
        <div className="panel bg-lime-dark px-6 py-16 text-lime-light md:px-14 md:py-24">
          <div className="flex flex-col items-center text-center">
            <p className="t-label text-lime">Why Hermes</p>
            <h2 className="t-heading-xl mt-5 text-lime">
              One question,
              <br />
              answered honestly
            </h2>
          </div>
          <div className="mt-12 grid gap-4 md:mt-16 md:grid-cols-2">
            {reasons.map((reason, index) => (
              <article
                key={reason.title}
                className="rounded-[24px] border border-lime/30 p-8 md:rounded-[36px] md:p-10"
              >
                <p className="t-label text-lime">
                  {String(index + 1).padStart(2, "0")}
                </p>
                <h3 className="t-heading mt-10 text-paper">{reason.title}</h3>
                <p className="t-body mt-4 max-w-[34rem]">{reason.body}</p>
              </article>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
