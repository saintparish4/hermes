import { PipelineDiagram } from "@/components/pipeline-diagram";

export function HowItWorks() {
  return (
    <section id="how-it-works" className="py-20 md:py-24">
      <div className="page-wrap">
        <p className="section-label">How Hermes works</p>
        <div className="mt-6 max-w-3xl">
          <h2 className="t-heading-lg text-off-black">
            Probe the slots. Walk the keys. Price the blast radius.
          </h2>
          <p className="t-body mt-6 max-w-2xl text-graphite">
            Every existing smart-contract tool analyzes code. Hermes ignores
            bytecode and models the capability surface instead: who can upgrade
            what, how many keys that takes, whether a timelock stands in the
            way, and what the resulting exposure is in dollars.
          </p>
        </div>
        <PipelineDiagram />
      </div>
    </section>
  );
}
