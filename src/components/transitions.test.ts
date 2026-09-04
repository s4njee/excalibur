// Transition discipline (E1-S5).
//
// The handoff is specific and short: 120ms for hover and disabled-state colour
// changes, 140ms for Quick Look, and *no layout animation in the list* —
// "it fights scanning". The last of those is the one worth a test, because it
// is a rule about what must not be added, and nothing else would catch it.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (name: string) =>
  readFileSync(resolve(process.cwd(), `src/components/${name}`), "utf8");

const SHELL = read("shell.module.css");
const TOKENS = readFileSync(resolve(process.cwd(), "src/tokens/tokens.css"), "utf8");

/** Every `selector { ... }` block in a stylesheet. */
function rules(css: string): { selector: string; body: string }[] {
  const found: { selector: string; body: string }[] = [];
  for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (selector && body) found.push({ selector: selector.trim(), body });
  }
  return found;
}

/**
 * Properties that move or resize something.
 *
 * `transition: all` counts too: it animates these without naming them, which
 * is exactly how a layout animation gets added by accident.
 */
const GEOMETRY = [
  "all",
  "width",
  "height",
  "top",
  "left",
  "right",
  "bottom",
  "margin",
  "padding",
  "transform",
  "grid-template-columns",
  "flex",
];

/** The list's own selectors — rows, cells, and the grid they sit in. */
const LIST = /\.(row|rows|nameCell|sizeCell|metaCell|name|virtualSpacer|grid|columnHeader|mono|nameLed)/i;

describe("transitions", () => {
  it("names the two durations the handoff gives, once each", () => {
    expect(TOKENS).toContain("--transition-state: 120ms ease");
    expect(TOKENS).toContain("--transition-panel: 140ms ease");
  });

  it("never animates row geometry", () => {
    // The rule the handoff states outright: layout animation in the list
    // fights scanning. A row that eases into place is a row the eye follows
    // instead of reading.
    for (const rule of rules(SHELL)) {
      if (!LIST.test(rule.selector)) continue;
      const transition = /transition:\s*([^;]*)/.exec(rule.body)?.[1];
      if (!transition) continue;
      for (const property of GEOMETRY) {
        expect(
          new RegExp(`(^|[\\s,])${property}([\\s,]|$)`).test(transition),
          `${rule.selector} animates ${property}: ${transition.trim()}`,
        ).toBe(false);
      }
    }
  });

  it("only transitions colour-ish properties anywhere in the shell", () => {
    // A softer version of the same rule for the chrome around the list, where
    // the handoff allows hover and disabled-state colour changes and nothing
    // else. `width` is permitted on the transfer progress bar alone, which
    // lives in another stylesheet.
    const allowed = ["color", "background", "background-color", "border-color", "opacity", "outline-color"];
    for (const rule of rules(SHELL)) {
      const transition = /transition:\s*([^;]*)/.exec(rule.body)?.[1];
      if (!transition) continue;
      for (const part of transition.split(",")) {
        const property = part.trim().split(/\s+/)[0];
        if (!property) continue;
        expect(allowed, `${rule.selector} transitions ${property}`).toContain(property);
      }
    }
  });

  it("uses the tokens rather than raw durations", () => {
    // A hardcoded `120ms` would drift the moment the token changed.
    for (const rule of rules(SHELL)) {
      const transition = /transition:\s*([^;]*)/.exec(rule.body)?.[1];
      if (!transition) continue;
      expect(/\d+m?s/.test(transition), `${rule.selector} hardcodes a duration`).toBe(false);
    }
  });
});
