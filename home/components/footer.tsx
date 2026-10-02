import {
  APP_URL,
  CHANGES_URL,
  LOOKUP_URL,
  METHODOLOGY_URL,
} from "@/components/links";
import { Logo } from "@/components/logo";

const columns = [
  {
    title: "Product",
    links: [
      { href: APP_URL, label: "The map" },
      { href: LOOKUP_URL, label: "Look up an address" },
      { href: CHANGES_URL, label: "Change feed" },
    ],
  },
  {
    title: "Method",
    links: [
      { href: METHODOLOGY_URL, label: "Methodology" },
      { href: "#how-it-works", label: "How it works" },
      { href: "#faq", label: "FAQ" },
    ],
  },
  {
    title: "Project",
    links: [
      { href: "#why", label: "Why Hermes" },
      { href: "mailto:hello@hermes.dev", label: "Email" },
    ],
  },
];

export function Footer() {
  return (
    <footer className="mt-12 overflow-hidden bg-ink pt-16 text-paper md:mt-20 md:pt-24">
      <div className="page-wrap">
        <div className="grid gap-12 md:grid-cols-[minmax(0,1.3fr)_repeat(3,minmax(0,0.7fr))]">
          <div>
            <Logo className="text-paper" />
            <p className="t-body mt-6 max-w-xs text-steel">
              The upgrade-authority graph for Base: who can change the code, and
              how many keys that takes.
            </p>
          </div>
          {columns.map((column) => (
            <div key={column.title}>
              <p className="t-label text-steel">{column.title}</p>
              <ul className="mt-5 flex flex-col gap-3">
                {column.links.map((link) => (
                  <li key={link.label}>
                    <a
                      className="text-[17px] text-paper no-underline hover:text-flame"
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
        <div className="mt-16 flex flex-col gap-3 border-t border-slate pt-8 sm:flex-row sm:items-center sm:justify-between">
          <p className="t-label text-steel">© 2026 Hermes. MIT License.</p>
          <p className="t-label text-steel">Capability, never intent.</p>
        </div>
        <p
          aria-hidden
          className="t-display mt-10 select-none text-center !text-[16vw] !leading-[0.74] whitespace-nowrap text-flame"
        >
          Hermes
        </p>
      </div>
    </footer>
  );
}
