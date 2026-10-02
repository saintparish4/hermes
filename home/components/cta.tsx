import { APP_URL, METHODOLOGY_URL } from "@/components/links";

export function Cta() {
  return (
    <section id="cta" className="py-4 md:py-8">
      <div className="page-wrap">
        <div className="panel bg-orchid px-6 py-16 text-center text-orchid-dark md:px-16 md:py-28">
          <p className="t-label">The map is public</p>
          <h2 className="t-display mt-6">
            Read
            <br />
            the map
          </h2>
          <p className="t-body-lg mx-auto mt-8 max-w-xl">
            No login and no sales wall. Look up any address on Base and see what
            stands behind its upgrade.
          </p>
          <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
            <a
              className="btn [--btn-bg:#3d065f] [--btn-fg:#eac2ff]"
              href={APP_URL}
            >
              Open the map
            </a>
            <a
              className="btn btn-outline [--btn-bg:#3d065f] [--btn-fg:#eac2ff]"
              href={METHODOLOGY_URL}
            >
              Methodology
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}
