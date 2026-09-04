import { describe, expect, it } from "vitest";
import { memoryPrefs } from "./prefs";
import {
  DEFAULTS,
  MAX_CONCURRENCY,
  loadSettings,
  resolveTheme,
  sanitiseField,
  saveSetting,
} from "./settings";

describe("settings", () => {
  it("round-trips every field through the store", async () => {
    const prefs = memoryPrefs();
    await saveSetting(prefs, "theme", "dark");
    await saveSetting(prefs, "showHidden", false);
    await saveSetting(prefs, "foldersFirst", false);
    await saveSetting(prefs, "sidebarCollapsed", true);
    await saveSetting(prefs, "downloadFolder", "/Users/dorian/Downloads");
    await saveSetting(prefs, "transferConcurrency", 5);
    await saveSetting(prefs, "conflictDefault", "keepBoth");
    await saveSetting(prefs, "sort", { column: "size", ascending: false });

    const settings = await loadSettings(prefs);
    expect(settings.theme).toBe("dark");
    expect(settings.showHidden).toBe(false);
    expect(settings.foldersFirst).toBe(false);
    expect(settings.sidebarCollapsed).toBe(true);
    expect(settings.downloadFolder).toBe("/Users/dorian/Downloads");
    expect(settings.transferConcurrency).toBe(5);
    expect(settings.conflictDefault).toBe("keepBoth");
    expect(settings.sort).toEqual({ column: "size", ascending: false });
  });

  it("falls back per field, not per record", async () => {
    // The file is user-editable and outlives upgrades. One bad value must cost
    // that value alone — discarding the record would silently reset a user's
    // whole setup because they mistyped one line.
    const prefs = memoryPrefs();
    await prefs.set("theme", "chartreuse");
    await prefs.set("showHidden", "yes please");
    await prefs.set("transferConcurrency", "lots");
    await prefs.set("foldersFirst", false);

    const settings = await loadSettings(prefs);
    expect(settings.theme).toBe(DEFAULTS.theme);
    expect(settings.showHidden).toBe(DEFAULTS.showHidden);
    expect(settings.transferConcurrency).toBe(DEFAULTS.transferConcurrency);
    // The one good value survived its neighbours.
    expect(settings.foldersFirst).toBe(false);
  });

  it("returns the defaults for a store that has never been written", async () => {
    expect(await loadSettings(memoryPrefs())).toEqual(DEFAULTS);
  });

  it("survives a store that throws", async () => {
    // A launch is not the moment to fail over an unreadable preference.
    const broken = {
      async get(): Promise<unknown> {
        throw new Error("no");
      },
      async set(): Promise<void> {},
    };
    expect(await loadSettings(broken)).toEqual(DEFAULTS);
  });

  it("clamps concurrency rather than rejecting it", () => {
    // Someone who edited the file to 40 meant "more"; the ceiling is the
    // nearest honest answer to that, and 0 would mean nothing ever runs.
    expect(sanitiseField("transferConcurrency", 40)).toBe(MAX_CONCURRENCY);
    expect(sanitiseField("transferConcurrency", 0)).toBe(1);
    expect(sanitiseField("transferConcurrency", -3)).toBe(1);
    expect(sanitiseField("transferConcurrency", 2.6)).toBe(3);
    expect(sanitiseField("transferConcurrency", Number.NaN)).toBe(DEFAULTS.transferConcurrency);
  });

  it("treats an empty download folder as no preference", () => {
    // `null` means "ask the engine for the OS folder", which is a different
    // thing from a path that happens to be blank.
    expect(sanitiseField("downloadFolder", "")).toBeNull();
    expect(sanitiseField("downloadFolder", "   ")).toBeNull();
    expect(sanitiseField("downloadFolder", 42)).toBeNull();
  });

  it("rejects a half-valid sort rather than half-applying it", () => {
    expect(sanitiseField("sort", { column: "size" })).toEqual(DEFAULTS.sort);
    expect(sanitiseField("sort", { column: "nonsense", ascending: true })).toEqual(DEFAULTS.sort);
    expect(sanitiseField("sort", { column: "kind", ascending: false })).toEqual({
      column: "kind",
      ascending: false,
    });
  });

  it("keeps the choice and the painted theme apart", () => {
    // They differ exactly when the choice is `system`, and conflating them
    // loses the ability to tell "picked light" from "the OS is light today".
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });

  it("reads the column widths from the key they already use", async () => {
    // Renaming this would silently discard widths users have already dragged.
    const prefs = memoryPrefs();
    await prefs.set("columnWidths.columns", { size: 120, modified: 150, kind: 130 });
    expect((await loadSettings(prefs)).columnWidths).toEqual({
      size: 120,
      modified: 150,
      kind: 130,
    });
  });
});
