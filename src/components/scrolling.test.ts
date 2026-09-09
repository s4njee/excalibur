// A flex column that grows must also be allowed to shrink.
//
// The trap is CSS's, not this app's: a flex item's `min-height` is `auto`, so a
// column with `flex: 1` refuses to become shorter than its content. It looks
// right until something above it takes a fixed height — and then the item
// overflows its parent instead of scrolling, and whatever sits *below* it is
// pushed out of the window.
//
// That is exactly how the directory tree band (XTP-E2-S4) broke the pane
// column: 104px of tree above a list that would not shrink meant the per-pane
// status bar left the bottom of the window in any short window. Nothing about
// the markup says so, and no test that renders in jsdom can see it — jsdom has
// no layout — so the rule is stated once and checked here.

import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const DIRECTORY = resolve(process.cwd(), "src/components");

/** Every `selector { body }` block, with comments stripped first. */
function rules(css: string): { selector: string; body: string }[] {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const found: { selector: string; body: string }[] = [];
  for (const [, selector, body] of stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (selector && body) found.push({ selector: selector.trim(), body });
  }
  return found;
}

/** A vertical flex container that takes the space it is given. */
function growsAsAColumn(body: string): boolean {
  const column =
    /display:\s*flex/.test(body) && /flex-direction:\s*column/.test(body);
  const grows = /\bflex:\s*1\b/.test(body) || /flex-grow:\s*[1-9]/.test(body);
  return column && grows;
}

describe("flex columns", () => {
  const sheets = readdirSync(DIRECTORY).filter((name) => name.endsWith(".module.css"));

  it("has stylesheets to check", () => {
    // Guards the test itself: a glob that matched nothing would pass forever.
    expect(sheets.length).toBeGreaterThan(3);
  });

  for (const sheet of sheets) {
    it(`lets every growing column shrink in ${sheet}`, () => {
      for (const rule of rules(readFileSync(resolve(DIRECTORY, sheet), "utf8"))) {
        if (!growsAsAColumn(rule.body)) continue;
        expect(
          /min-height:\s*0/.test(rule.body),
          `${sheet} ${rule.selector} grows as a column without min-height: 0, ` +
            `so its content cannot scroll and what is below it leaves the window`,
        ).toBe(true);
      }
    });
  }
});
