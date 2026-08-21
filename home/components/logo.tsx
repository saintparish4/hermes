export function Logo({ className = "" }: { className?: string }) {
  return (
    <a
      href="#top"
      className={`inline-flex items-center gap-2.5 text-off-black no-underline ${className}`}
    >
      <span
        aria-hidden
        className="size-2.5 rounded-full bg-off-black"
      />
      <span className="font-serif text-[22px] leading-none tracking-[-0.44px]">
        Hermes
      </span>
    </a>
  );
}
