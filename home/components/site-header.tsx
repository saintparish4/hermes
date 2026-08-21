"use client";

import { useState } from "react";
import { Logo } from "@/components/logo";

const navItems = [
  { href: "#how-it-works", label: "Product" },
  { href: "#connect", label: "Connect" },
  { href: "#why", label: "Why Hermes" },
  { href: "#faq", label: "FAQ" },
];

export function SiteHeader() {
  const [announceOpen, setAnnounceOpen] = useState(true);
  const [menuOpen, setMenuOpen] = useState(false);

  function closeMenu() {
    setMenuOpen(false);
  }

  return (
    <header className="sticky top-0 z-50 bg-parchment">
      {announceOpen ? (
        <div className="flex h-10 items-center bg-ink text-parchment">
          <div className="page-wrap flex w-full items-center justify-between gap-4">
            <p className="t-button truncate text-parchment">
              Now indexing ERC-1967 proxies on Base — coverage published, not
              estimated.
            </p>
            <div className="flex shrink-0 items-center gap-3">
              <a
                href="#faq"
                className="hidden h-7 items-center rounded-full border border-parchment px-3 text-[12px] uppercase tracking-[-0.4px] text-parchment no-underline sm:inline-flex"
              >
                Read the note
              </a>
              <button
                type="button"
                className="icon-close px-1 text-[18px] leading-none text-parchment"
                onClick={() => setAnnounceOpen(false)}
                aria-label="Dismiss announcement"
              >
                ×
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <nav className="page-wrap flex h-20 items-center justify-between gap-6">
        <Logo />

        <ul className="hidden items-center gap-8 lg:flex">
          {navItems.map((item) => (
            <li key={item.href}>
              <a className="nav-link" href={item.href}>
                {item.label}
              </a>
            </li>
          ))}
        </ul>

        <div className="hidden items-center gap-3 sm:flex">
          <a className="btn-ghost" href="#faq">
            Methodology
          </a>
          <a className="btn-primary" href="#cta">
            Get a Demo <span aria-hidden>▸</span>
          </a>
        </div>

        <div className="relative lg:hidden">
          <button
            type="button"
            className="t-button flex h-12 items-center rounded-[100px] border border-off-black px-6 text-off-black"
            aria-expanded={menuOpen}
            aria-controls="mobile-nav"
            onClick={() => setMenuOpen((open) => !open)}
          >
            {menuOpen ? "Close" : "Menu"}
          </button>
          {menuOpen ? (
            <div
              id="mobile-nav"
              className="absolute right-0 top-[calc(100%+8px)] w-64 rounded-[40px] border border-ash bg-parchment p-8"
            >
              <ul className="flex flex-col gap-5">
                {navItems.map((item) => (
                  <li key={item.href}>
                    <a className="nav-link" href={item.href} onClick={closeMenu}>
                      {item.label}
                    </a>
                  </li>
                ))}
              </ul>
              <div className="mt-8 flex flex-col gap-3 sm:hidden">
                <a className="btn-ghost w-full" href="#faq" onClick={closeMenu}>
                  Methodology
                </a>
                <a className="btn-primary w-full" href="#cta" onClick={closeMenu}>
                  Get a Demo <span aria-hidden>▸</span>
                </a>
              </div>
            </div>
          ) : null}
        </div>
      </nav>
    </header>
  );
}
