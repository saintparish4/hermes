import { Suspense } from "react";
import { ConnectEverything } from "@/components/connect-everything";
import { Cta } from "@/components/cta";
import { Faq } from "@/components/faq";
import { Footer } from "@/components/footer";
import { Hero } from "@/components/hero";
import { HowItWorks } from "@/components/how-it-works";
import { SiteHeader } from "@/components/site-header";
import { TrustedBy } from "@/components/trusted-by";
import { WhyTeams } from "@/components/why-teams";

export default function Home() {
  return (
    <>
      <a
        href="#content"
        className="sr-only focus:not-sr-only focus:absolute focus:left-6 focus:top-6 focus:z-[60] focus:rounded-[100px] focus:bg-off-black focus:px-6 focus:py-3 focus:text-parchment"
      >
        Skip to content
      </a>
      <Suspense fallback={<div className="h-[120px] bg-parchment" />}>
        <SiteHeader />
      </Suspense>
      <main id="content" className="flex-1">
        <Hero />
        <TrustedBy />
        <HowItWorks />
        <ConnectEverything />
        <WhyTeams />
        <Faq />
        <Cta />
      </main>
      <Footer />
    </>
  );
}
