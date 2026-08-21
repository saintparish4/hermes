export function Cta() {
  return (
    <section id="cta" className="py-20 md:py-24">
      <div className="page-wrap">
        <div className="relative overflow-hidden rounded-[40px] bg-periwinkle-mist px-8 py-16 text-center md:px-16 md:py-24">
          <div className="pointer-events-none absolute inset-0" aria-hidden>
            <div className="wash -left-10 top-0 h-56 w-72 bg-[radial-gradient(ellipse_at_center,rgba(255,148,115,0.7)_0%,transparent_70%)]" />
            <div className="wash -right-8 bottom-0 h-56 w-80 bg-[radial-gradient(ellipse_at_center,rgba(167,252,205,0.8)_0%,rgba(160,181,235,0.6)_40%,transparent_70%)]" />
          </div>
          <div className="relative">
            <p className="section-label">The map is public</p>
            <h2 className="t-heading-lg mx-auto mt-6 max-w-[18ch] text-off-black">
              Walk the authority graph with us.
            </h2>
            <p className="t-body-lg mx-auto mt-6 max-w-xl text-graphite">
              No login. No sales wall. If you run a protocol on Base and want a
              hand-verified pass against the scanner, say hello.
            </p>
            <div className="mt-10 flex flex-wrap items-center justify-center gap-4">
              <a className="btn-primary" href="mailto:hello@hermes.dev">
                Get a Demo <span aria-hidden>▸</span>
              </a>
              <a className="btn-dark" href="#how-it-works">
                View the method
              </a>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
