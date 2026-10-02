import { Cta } from "@/components/cta";
import { Faq } from "@/components/faq";
import { Features } from "@/components/features";
import { Footer } from "@/components/footer";
import { Hero } from "@/components/hero";
import { HowItWorks } from "@/components/how-it-works";
import { Reads } from "@/components/reads";
import { SiteHeader } from "@/components/site-header";
import { WhyTeams } from "@/components/why-teams";

export default function Home() {
  return (
    <>
      <a
        href="#content"
        className="sr-only focus:not-sr-only focus:absolute focus:left-6 focus:top-6 focus:z-[60] focus:rounded-full focus:bg-ink focus:px-6 focus:py-3 focus:text-paper"
      >
        Skip to content
      </a>
      <SiteHeader />
      <main id="content" className="flex-1">
        <Hero />
        <Reads />
        <HowItWorks />
        <Features />
        <WhyTeams />
        <Faq />
        <Cta />
      </main>
      <Footer />
    </>
  );
}
