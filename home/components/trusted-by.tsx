const teams = [
  "Aerodrome",
  "Moonwell",
  "Morpho",
  "Seamless",
  "Compound",
  "Uniswap",
  "Aave",
];

export function TrustedBy() {
  return (
    <section className="pb-20 pt-4 md:pb-24" aria-labelledby="trusted-by-label">
      <div className="page-wrap">
        <p id="trusted-by-label" className="section-label">
          Trusted by blockchain teams at
        </p>
        <ul className="mt-8 flex flex-wrap items-center gap-x-8 gap-y-5 md:gap-x-12">
          {teams.map((name) => (
            <li
              key={name}
              className="font-mono text-[18px] uppercase tracking-[-0.36px] text-smoke"
            >
              {name}
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
