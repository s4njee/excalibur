import { describe, expect, it } from "vitest";
import { filterEntries, fold, matches } from "./filter";
import type { Entry } from "./types";

const entry = (name: string): Entry => ({
  name,
  path: name,
  kind: "file",
  size: 0,
  modifiedMs: null,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Document",
  kindCategory: "other",
  hidden: false,
});

describe("fold", () => {
  it("lowercases", () => {
    expect(fold("Arrival")).toBe("arrival");
  });

  it("strips combining accents", () => {
    expect(fold("Résumé")).toBe("resume");
    expect(fold("Ångström")).toBe("angstrom");
  });

  it("handles letters that NFD will not decompose", () => {
    // `ø` is its own letter, not `o` plus a mark, so normalisation alone
    // leaves it — and `soren` would not find `Søren`.
    expect(fold("Søren")).toBe("soren");
    expect(fold("Straße")).toBe("strasse");
    expect(fold("Æon")).toBe("aeon");
  });
});

describe("matches", () => {
  it("is a substring match, not a prefix match", () => {
    expect(matches("Blade.Runner.2049.mkv", "runner")).toBe(true);
  });

  it("ignores case in either direction", () => {
    expect(matches("ARRIVAL.mkv", "arrival")).toBe(true);
    expect(matches("arrival.mkv", "ARRIVAL")).toBe(true);
  });

  it("ignores diacritics in either direction", () => {
    // Typing plain letters finds the accented file...
    expect(matches("Amélie.mkv", "amelie")).toBe(true);
    // ...and typing the accented form finds it too.
    expect(matches("Amelie.mkv", "Amélie")).toBe(true);
  });

  it("matches everything on an empty filter", () => {
    expect(matches("anything", "")).toBe(true);
  });

  it("does not match what is not there", () => {
    expect(matches("Arrival.mkv", "solaris")).toBe(false);
  });
});

describe("filterEntries", () => {
  it("returns the listing untouched when nothing is typed", () => {
    const entries = [entry("a"), entry("b")];
    expect(filterEntries(entries, "")).toBe(entries);
    // Whitespace alone is not a filter anyone meant to type.
    expect(filterEntries(entries, "   ")).toBe(entries);
  });

  it("keeps only what matches", () => {
    const entries = [entry("Arrival.2016.mkv"), entry("Arrival.2016.srt"), entry("Moon.2009.mp4")];
    expect(filterEntries(entries, "arrival").map((e) => e.name)).toEqual([
      "Arrival.2016.mkv",
      "Arrival.2016.srt",
    ]);
  });
});
