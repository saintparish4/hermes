const reads = [
  "ERC-1967",
  "UUPS",
  "Beacon",
  "EIP-1822",
  "ProxyAdmin",
  "Safe",
  "Timelock",
  "AccessControl",
  "ERC-4337",
  "EIP-7702",
  "L1→L2 alias",
];

function Row({ hidden = false }: { hidden?: boolean }) {
  return (
    <ul
      className="flex shrink-0 items-center"
      aria-hidden={hidden || undefined}
    >
      {reads.map((name) => (
        <li
          key={name}
          className="t-heading flex items-center whitespace-nowrap after:mx-8 after:text-flame after:content-['✦']"
        >
          {name}
        </li>
      ))}
    </ul>
  );
}

export function Reads() {
  return (
    <section
      className="overflow-hidden py-10 md:py-14"
      aria-labelledby="reads-label"
    >
      <p id="reads-label" className="sr-only">
        What Hermes reads
      </p>
      <div className="flex w-max animate-ticker">
        <Row />
        <Row hidden />
      </div>
    </section>
  );
}
