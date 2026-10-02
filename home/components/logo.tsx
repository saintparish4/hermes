export function Logo({ className = "" }: { className?: string }) {
  return (
    <a
      href="#top"
      className={`inline-flex items-center gap-2.5 no-underline ${className}`}
    >
      <svg width="30" height="30" viewBox="0 0 30 30" fill="none" aria-hidden>
        <rect width="30" height="30" rx="9" fill="#ff5c16" />
        <path d="M9 9.5 15 20.5 21 9.5" stroke="#0a0a0a" strokeWidth="2" />
        <circle cx="9" cy="9.5" r="3" fill="#0a0a0a" />
        <circle cx="21" cy="9.5" r="3" fill="#0a0a0a" />
        <circle
          cx="15"
          cy="20.5"
          r="3"
          fill="#ffffff"
          stroke="#0a0a0a"
          strokeWidth="2"
        />
      </svg>
      <span className="font-display text-[22px] font-black uppercase leading-none tracking-[-0.02em] [font-stretch:125%]">
        Hermes
      </span>
    </a>
  );
}
