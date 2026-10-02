import { CHANGES_URL, LOOKUP_URL } from "@/components/links";

const features = [
  {
    label: "History",
    title: "Changes are recorded",
    body: "The graph is kept between scans. When a root changes hands or a threshold moves, Hermes records it and pins it to a block where it can. History starts when Hermes first sees an address.",
    tone: "bg-flame-light text-flame-dark",
    span: "lg:col-span-2",
    button: "[--btn-bg:#661800] [--btn-fg:#ffa680]",
    link: { href: CHANGES_URL, label: "Change feed" },
  },
  {
    label: "Reach",
    title: "What else a key touches",
    body: "For any key or Safe: the proxies it controls alone, the ones it takes part in, and the ones Hermes cannot tell. The three are never added together.",
    tone: "bg-sky-light text-sky-dark",
    span: "",
  },
  {
    label: "Signers",
    title: "Reuse across Safes",
    body: "Which Safes share signers, so two authorities that look separate can be seen to rest on the same people.",
    tone: "bg-orchid-light text-orchid-dark",
    span: "",
  },
  {
    label: "Policy",
    title: "A check for your CI",
    body: "Write the keys and delay your deployments must have. hermes check fails the build when they do not, and fails closed on unknown.",
    tone: "bg-lime-light text-lime-dark",
    span: "",
  },
  {
    label: "API",
    title: "Every answer carries its scope",
    body: "JSON over /v1, no login. Each response says which scan it came from and that the index is a sample.",
    tone: "bg-fog text-ink",
    span: "",
    button: "",
    link: { href: LOOKUP_URL, label: "Look up an address" },
  },
];

export function Features() {
  return (
    <section id="features" className="py-16 md:py-24">
      <div className="page-wrap">
        <h2 className="t-heading-xl">
          One row
          <br />
          per <span className="text-flame">authority</span>
        </h2>
        <p className="t-body-lg mt-7 max-w-[42rem] text-slate">
          Two proxies under different ProxyAdmins owned by one Safe are one row:
          that Safe. Grouping on the immediate admin splits the picture and
          hides how much rests on a single authority.
        </p>
        <div className="mt-12 grid gap-4 md:mt-16 md:grid-cols-2 lg:grid-cols-3">
          {features.map((feature) => (
            <article
              key={feature.label}
              className={`panel flex min-h-[300px] flex-col justify-between p-8 md:p-10 ${feature.tone} ${feature.span}`}
            >
              <p className="t-label">{feature.label}</p>
              <div className="mt-12">
                <h3 className="t-heading">{feature.title}</h3>
                <p className="t-body mt-4 max-w-[38rem]">{feature.body}</p>
                {feature.link ? (
                  <a
                    className={`btn mt-7 ${feature.button}`}
                    href={feature.link.href}
                  >
                    {feature.link.label}
                  </a>
                ) : null}
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
