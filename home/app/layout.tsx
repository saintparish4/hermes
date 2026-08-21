import type { Metadata } from "next";
import { IBM_Plex_Mono, Instrument_Serif } from "next/font/google";
import "./globals.css";

const untitledSerif = Instrument_Serif({
  weight: "400",
  subsets: ["latin"],
  variable: "--font-untitled-serif",
  display: "swap",
});

const abcDiatypeMono = IBM_Plex_Mono({
  weight: ["400", "500"],
  subsets: ["latin"],
  variable: "--font-abc-diatype-mono",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Hermes — Rank the keys, not the contracts",
  description:
    "If this authority is compromised tomorrow, how much money moves, and how long do you have to react? Hermes maps privileged keys on Base by dollar exposure.",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="en"
      className={`${untitledSerif.variable} ${abcDiatypeMono.variable} h-full antialiased`}
    >
      <body className="flex min-h-full flex-col bg-parchment font-mono text-off-black">
        {children}
      </body>
    </html>
  );
}
