// Every full-window overlay declares where it sits in the stack.
//
// This exists because of a bug that was invisible in review and obvious the
// moment anything was clicked (2026-09-05): `dialog.module.css`'s `.scrim` was
// `position: fixed` with no `z-index`, and the file list's rows live inside a
// `transform` — which puts them in the same painting layer as a positioned
// element. Which one won then came down to tree order, so the New Connection
// dialog (mounted after the list) looked right while the Properties and
// Move to… sheets (mounted before it) rendered *behind* the rows, with their
// buttons unclickable and only the gaps between columns showing through.
//
// Stacking that depends on where a component happens to be mounted is not a
// property anyone can review, so the rule is stated once and checked here.

import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const DIRECTORY = resolve(process.cwd(), "src/components");

/** Every `selector { body }` block in a stylesheet. */
function rules(css: string): { selector: string; body: string }[] {
  const found: { selector: string; body: string }[] = [];
  for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (selector && body) found.push({ selector: selector.trim(), body });
  }
  return found;
}

/**
 * A rule that covers the window: fixed or absolute, pinned to all four edges.
 *
 * That is the shape of a scrim, and the only shape where being painted under
 * the content is both possible and silent — a mis-stacked *element* is
 * visibly in the wrong place, while a mis-stacked overlay looks almost right.
 */
function coversTheWindow(body: string): boolean {
  const positioned = /position:\s*(fixed|absolute)/.test(body);
  // All four edges, not three: the `..` row is pinned left/right/top and is a
  // row, not an overlay. `bottom` is what makes a rule a covering.
  const edges = ["top", "right", "bottom", "left"];
  const pinned =
    /inset:\s*0/.test(body) ||
    edges.every((edge) => new RegExp(`${edge}:\\s*0`).test(body));
  return positioned && pinned;
}

describe("overlays", () => {
  const sheets = readdirSync(DIRECTORY).filter((name) => name.endsWith(".module.css"));

  it("has stylesheets to check", () => {
    // Guards the test itself: a glob that matched nothing would pass forever.
    expect(sheets.length).toBeGreaterThan(3);
  });

  for (const sheet of sheets) {
    it(`declares a z-index for every full-window overlay in ${sheet}`, () => {
      const css = readFileSync(resolve(DIRECTORY, sheet), "utf8");
      for (const rule of rules(css)) {
        if (!coversTheWindow(rule.body)) continue;
        expect(
          /z-index:\s*-?\d+/.test(rule.body),
          `${sheet} ${rule.selector} covers the window with no z-index, so what it ` +
            `covers depends on where it is mounted`,
        ).toBe(true);
      }
    });
  }
});
