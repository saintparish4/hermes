"use client";

import { useState } from "react";
import { APP_URL } from "@/components/links";
import { Logo } from "@/components/logo";

const navItems = [
  { href: "#how-it-works", label: "How it works" },
  { href: "#features", label: "Features" },
  { href: "#why", label: "Why Hermes" },
  { href: "#faq", label: "FAQ" },
];

export function SiteHeader() {
  const [menuOpen, setMenuOpen] = useState(false);

  function closeMenu() {
    setMenuOpen(false);
  }

  return (
    <header className="sticky top-0 z-50 bg-paper/90 backdrop-blur-md">
      <nav className="page-wrap flex h-[72px] items-center justify-between gap-6 md:h-20">
        <Logo className="text-ink" />

        <ul className="hidden items-center gap-9 lg:flex">
          {navItems.map((item) => (
            <li key={item.href}>
              <a className="nav-link" href={item.href}>
                {item.label}
              </a>
            </li>
          ))}
        </ul>

        <div className="flex items-center gap-3">
          <a className="btn hidden sm:inline-flex" href={APP_URL}>
            Open the map
          </a>
          <button
            type="button"
            className="btn btn-outline lg:hidden"
            aria-expanded={menuOpen}
            aria-controls="mobile-nav"
            onClick={() => setMenuOpen((open) => !open)}
          >
            {menuOpen ? "Close" : "Menu"}
          </button>
        </div>
      </nav>

      {menuOpen ? (
        <div id="mobile-nav" className="page-wrap pb-4 lg:hidden">
          <div className="rounded-[28px] bg-ink p-8 text-paper">
            <ul className="flex flex-col gap-5">
              {navItems.map((item) => (
                <li key={item.href}>
                  <a
                    className="t-heading text-paper no-underline"
                    href={item.href}
                    onClick={closeMenu}
                  >
                    {item.label}
                  </a>
                </li>
              ))}
            </ul>
            <a
              className="btn mt-8 w-full [--btn-bg:#ff5c16] [--btn-fg:#0a0a0a]"
              href={APP_URL}
              onClick={closeMenu}
            >
              Open the map
            </a>
          </div>
        </div>
      ) : null}
    </header>
  );
}
