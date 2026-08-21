type NodeSpec = {
  id: string;
  label: string;
  icon: "diamond" | "hash" | "shield" | "list" | "brackets" | "chart";
};

const sources: NodeSpec[] = [
  { id: "erc1967", label: "ERC-1967", icon: "diamond" },
  { id: "safes", label: "Safes", icon: "shield" },
  { id: "timelocks", label: "Timelocks", icon: "hash" },
];

const destinations: NodeSpec[] = [
  { id: "board", label: "Leaderboard", icon: "list" },
  { id: "api", label: "JSON API", icon: "brackets" },
  { id: "coverage", label: "Coverage", icon: "chart" },
];

function NodeIcon({ name }: { name: NodeSpec["icon"] }) {
  const common = {
    width: 12,
    height: 12,
    viewBox: "0 0 12 12",
    fill: "none",
    "aria-hidden": true as const,
  };

  switch (name) {
    case "diamond":
      return (
        <svg {...common}>
          <path d="M6 1.2 10.5 6 6 10.8 1.5 6Z" stroke="#242424" strokeWidth="1" />
        </svg>
      );
    case "hash":
      return (
        <svg {...common}>
          <path
            d="M4.2 1.5 3.2 10.5M8.8 1.5 7.8 10.5M1.8 4.4h8.4M1.8 7.6h8.4"
            stroke="#242424"
            strokeWidth="1"
          />
        </svg>
      );
    case "shield":
      return (
        <svg {...common}>
          <path
            d="M6 1.4 10.2 3.1v3.2c0 2.4-1.8 3.8-4.2 4.4C3.6 10.1 1.8 8.7 1.8 6.3V3.1L6 1.4Z"
            stroke="#242424"
            strokeWidth="1"
          />
        </svg>
      );
    case "list":
      return (
        <svg {...common}>
          <path
            d="M2 3h8M2 6h8M2 9h5"
            stroke="#242424"
            strokeWidth="1"
            strokeLinecap="round"
          />
        </svg>
      );
    case "brackets":
      return (
        <svg {...common}>
          <path
            d="M4.2 2.2H2.4v7.6h1.8M7.8 2.2h1.8v7.6H7.8"
            stroke="#242424"
            strokeWidth="1"
          />
        </svg>
      );
    case "chart":
      return (
        <svg {...common}>
          <path
            d="M2 9.5V7.2M6 9.5V3.4M10 9.5V5.6"
            stroke="#242424"
            strokeWidth="1"
            strokeLinecap="round"
          />
        </svg>
      );
  }
}

function PipelineNode({ node }: { node: NodeSpec }) {
  return (
    <span className="inline-flex items-center gap-2 rounded-full border border-ash bg-parchment px-5 py-2.5 font-mono text-[14px] uppercase tracking-[-0.28px] text-off-black">
      <NodeIcon name={node.icon} />
      {node.label}
    </span>
  );
}

export function PipelineDiagram() {
  return (
    <div className="relative mt-16">
      <div className="md:hidden flex flex-col items-center gap-4">
        {sources.map((node) => (
          <PipelineNode key={node.id} node={node} />
        ))}
        <span className="text-ash" aria-hidden>
          ↓
        </span>
        <span className="relative z-10 inline-flex items-center rounded-full border border-ash bg-parchment px-6 py-3 font-mono text-[14px] font-medium uppercase tracking-[-0.28px] text-off-black">
          <span
            className="hub-glow absolute left-1/2 top-1/2 -z-10 size-24 -translate-x-1/2 -translate-y-1/2 rounded-full bg-mint/80 blur-[28px]"
            aria-hidden
          />
          Hermes
        </span>
        <span className="text-ash" aria-hidden>
          ↓
        </span>
        {destinations.map((node) => (
          <PipelineNode key={node.id} node={node} />
        ))}
      </div>

      <div className="relative hidden md:block">
        <span
          className="hub-glow absolute left-1/2 top-1/2 size-40 -translate-x-1/2 -translate-y-1/2 rounded-full bg-mint/70 blur-[50px]"
          aria-hidden
        />

        <svg
          className="absolute inset-0 h-full w-full"
          viewBox="0 0 100 100"
          preserveAspectRatio="none"
          fill="none"
          aria-hidden
        >
          <path
            d="M16.7 14 C16.7 32, 50 30, 50 42"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d="M50 14 C50 28, 50 34, 50 42"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d="M83.3 14 C83.3 32, 50 30, 50 42"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d="M50 58 C50 70, 16.7 68, 16.7 86"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d="M50 58 C50 70, 50 76, 50 86"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d="M50 58 C50 70, 83.3 68, 83.3 86"
            stroke="#cecac8"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
        </svg>

        <div className="relative grid grid-rows-3 gap-y-16 py-2">
          <div className="grid grid-cols-3 justify-items-center">
            {sources.map((node) => (
              <PipelineNode key={node.id} node={node} />
            ))}
          </div>
          <div className="flex justify-center">
            <span className="relative z-10 inline-flex items-center rounded-full border border-ash bg-parchment px-7 py-3 font-mono text-[14px] font-medium uppercase tracking-[-0.28px] text-off-black">
              Hermes
            </span>
          </div>
          <div className="grid grid-cols-3 justify-items-center">
            {destinations.map((node) => (
              <PipelineNode key={node.id} node={node} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
