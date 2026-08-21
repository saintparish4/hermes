import { Logo } from "@/components/logo";

const columns = [
  {
    title: "Product",
    links: [
      { href: "#how-it-works", label: "How it works" },
      { href: "#connect", label: "Connect" },
      { href: "#why", label: "Why Hermes" },
      { href: "#cta", label: "Get a Demo" },
    ],
  },
  {
    title: "Method",
    links: [
      { href: "#faq", label: "FAQ" },
      { href: "#faq", label: "Coverage" },
      { href: "#faq", label: "Unknowns" },
      { href: "#how-it-works", label: "Pipeline" },
    ],
  },
  {
    title: "Project",
    links: [
      { href: "#why", label: "Why Hermes" },
      { href: "#faq", label: "Limitations" },
      { href: "#cta", label: "Contact" },
      { href: "mailto:hello@hermes.dev", label: "Email" },
    ],
  },
];

export function Footer() {
  return (
    <footer className="border-t border-ash pb-16 pt-16">
      <div className="page-wrap">
        <div className="grid gap-12 md:grid-cols-[minmax(0,1.2fr)_repeat(3,minmax(0,0.7fr))]">
          <div>
            <Logo />
            <p className="t-body mt-6 max-w-xs text-graphite">
              A public map of privileged authority on Base. Rank the keys, not
              the contracts.
            </p>
          </div>
          {columns.map((column) => (
            <div key={column.title}>
              <p className="t-caption text-smoke">{column.title}</p>
              <ul className="mt-5 flex flex-col gap-3">
                {column.links.map((link) => (
                  <li key={link.label}>
                    <a
                      className="t-button text-off-black no-underline hover:opacity-70"
                      href={link.href}
                    >
                      {link.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
        <div className="mt-16 flex flex-col gap-3 border-t border-ash pt-8 sm:flex-row sm:items-center sm:justify-between">
          <p className="t-caption text-smoke">
            © 2026 Hermes. MIT License.
          </p>
          <p className="t-caption text-smoke">Base-native. Coverage published.</p>
        </div>
      </div>
    </footer>
  );
}
