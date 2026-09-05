// The Go to Folder path rules (E26-S2).
//
// Two things are worth pinning here. A typed path is not a share-relative one
// until `resolve` has had it, and the ways it differs — a leading slash, a
// trailing one, `..` — are exactly the ways a user types. And Tab must never
// throw away a keystroke: completing to the first match would be a gamble, and
// a second Tab that undid the first would be worse.

import { describe, expect, it } from "vitest";
import { candidates, complete, parentOf, resolve, stemOf } from "./goto";

const NAMES = ["docs", "documents", "downloads", "Media", "archive"];

describe("resolve", () => {
  it("takes a share-relative path as it is", () => {
    expect(resolve("docs/2024")).toBe("docs/2024");
    expect(resolve("")).toBe("");
  });

  it("reads a leading slash as the share root, not the machine's", () => {
    // The path field shows `smb://host/media/docs`, so a user reaching for
    // "the top" types a slash. There is nothing outside the mount to mean.
    expect(resolve("/docs/2024")).toBe("docs/2024");
    expect(resolve("/")).toBe("");
  });

  it("tolerates the slashes of someone still typing", () => {
    expect(resolve("docs/")).toBe("docs");
    expect(resolve("docs//2024")).toBe("docs/2024");
    expect(resolve("  docs/2024  ")).toBe("docs/2024");
  });

  it("cannot climb above the share", () => {
    expect(resolve("docs/2024/..")).toBe("docs");
    expect(resolve("docs/./2024")).toBe("docs/2024");
    // The one that matters: `..` at the root stays at the root rather than
    // handing the engine `../etc` to refuse.
    expect(resolve("../../etc")).toBe("etc");
    expect(resolve("..")).toBe("");
  });
});

describe("stemOf and parentOf", () => {
  it("splits what is being completed from where it is completed", () => {
    expect(parentOf("do")).toBe("");
    expect(stemOf("do")).toBe("do");
    expect(parentOf("docs/re")).toBe("docs");
    expect(stemOf("docs/re")).toBe("re");
  });

  it("treats a trailing slash as an empty stem in that folder", () => {
    expect(parentOf("docs/")).toBe("docs");
    expect(stemOf("docs/")).toBe("");
  });

  it("resolves the parent, so completion works under a typed `..`", () => {
    expect(parentOf("/docs/2024/../re")).toBe("docs");
  });
});

describe("complete", () => {
  it("completes a single match and adds the separator", () => {
    // The next thing after completing a folder is descending into it.
    expect(complete("arc", NAMES)).toBe("archive/");
    expect(complete("docs/dow", ["downloads", "drafts"])).toBe("docs/downloads/");
  });

  it("extends only as far as the candidates agree", () => {
    // `d` matches three; `do` is as far as all three agree. Picking the first
    // would make Tab a gamble rather than a shortcut.
    expect(complete("d", NAMES)).toBe("do");
    expect(complete("docu", NAMES)).toBe("documents/");
  });

  it("says nothing happened rather than returning the input back", () => {
    // A second Tab on a stem already at the limit of agreement must be inert,
    // not a change the field then has to detect by comparing strings.
    expect(complete("do", NAMES)).toBeNull();
    expect(complete("zzz", NAMES)).toBeNull();
  });

  it("matches case- and diacritic-insensitively, and completes to the real name", () => {
    expect(complete("med", NAMES)).toBe("Media/");
    expect(complete("resume", ["Résumés"])).toBe("Résumés/");
  });

  it("keeps everything already typed, including a leading slash", () => {
    expect(complete("/docs/", ["2024"])).toBe("/docs/2024/");
    expect(complete("/arc", NAMES)).toBe("/archive/");
  });

  it("offers the whole folder when nothing is typed yet", () => {
    expect(candidates("docs/", NAMES)).toEqual(NAMES);
    expect(complete("docs/", ["only"])).toBe("docs/only/");
  });
});
