export function Hero() {
  return (
    <section
      id="top"
      className="relative overflow-hidden pb-20 pt-16 md:pb-24 md:pt-24"
    >
      <div className="pointer-events-none absolute inset-0" aria-hidden>
        <div className="wash left-1/2 top-8 h-[380px] w-[min(90vw,820px)] -translate-x-1/2 bg-[radial-gradient(ellipse_at_center,rgba(255,148,115,0.8)_0%,rgba(160,181,235,0.75)_42%,transparent_70%)] opacity-70" />
        <div className="wash right-[8%] top-32 h-[240px] w-[280px] bg-[radial-gradient(ellipse_at_center,rgba(167,252,205,0.85)_0%,transparent_70%)] opacity-60" />
      </div>

      <div className="page-wrap relative flex flex-col items-center text-center">
        <h1 className="t-display max-w-[18ch] text-off-black">
          If this key falls, how much moves?
        </h1>
        <p className="t-body-lg mt-8 max-w-[42rem] text-graphite">
          Hermes ranks every privileged authority on Base by the dollars it can
          spend — not the contract it sits on. One public map of who can
          upgrade what, how many keys that takes, and how long you have to
          react.
        </p>
        <div className="mt-10 flex flex-wrap items-center justify-center gap-4">
          <a className="btn-primary" href="#cta">
            Get a Demo <span aria-hidden>▸</span>
          </a>
          <a className="btn-ghost" href="#how-it-works">
            How it works
          </a>
        </div>
      </div>
    </section>
  );
}
