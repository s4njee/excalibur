import { describe, expect, it } from "vitest";
import { SHORTCUTS, hint, match, tooltip, type ShortcutId } from "./shortcuts";

/** A keystroke, as the browser would report it. */
function press(key: string, modifiers: { mod?: boolean; shift?: boolean } = {}): KeyboardEvent {
  return {
    key,
    metaKey: modifiers.mod ?? false,
    ctrlKey: false,
    shiftKey: modifiers.shift ?? false,
  } as KeyboardEvent;
}

describe("shortcuts", () => {
  it("maps the design's keys to their verbs", () => {
    expect(match(press(" "))).toBe("quickLook");
    expect(match(press("Enter"))).toBe("rename");
    expect(match(press("Escape"))).toBe("escape");
    expect(match(press("d", { mod: true }))).toBe("download");
    expect(match(press("u", { mod: true }))).toBe("upload");
    expect(match(press("e", { mod: true }))).toBe("edit");
    expect(match(press("r", { mod: true }))).toBe("refresh");
    expect(match(press("f", { mod: true }))).toBe("focusFilter");
    expect(match(press("[", { mod: true }))).toBe("back");
    expect(match(press("]", { mod: true }))).toBe("forward");
    expect(match(press(",", { mod: true }))).toBe("preferences");
    expect(match(press("ArrowDown", { mod: true }))).toBe("descend");
    expect(match(press("ArrowUp", { mod: true }))).toBe("ascend");
    expect(match(press("n", { mod: true, shift: true }))).toBe("newFolder");
    expect(match(press("t", { mod: true, shift: true }))).toBe("transfers");
  });

  it("accepts either case, because Shift changes the reported key", () => {
    // With Shift held the browser reports "N", not "n". Matching the literal
    // would make ⌘⇧N unreachable on every platform.
    expect(match(press("N", { mod: true, shift: true }))).toBe("newFolder");
    expect(match(press("T", { mod: true, shift: true }))).toBe("transfers");
  });

  it("matches the modifiers exactly rather than at least", () => {
    // ⌘N is not New Folder, and must not fall through to it.
    expect(match(press("n", { mod: true }))).toBeNull();
    // Nor does a bare letter mean anything — that is type-to-select.
    expect(match(press("d"))).toBeNull();
    // Nor does Shift alone turn a plain key into a verb.
    expect(match(press("Enter", { shift: true }))).toBeNull();
  });

  it("treats Backspace as Delete", () => {
    // Two keys, one verb — Mac keyboards label the same key differently.
    expect(match(press("Delete"))).toBe("delete");
    expect(match(press("Backspace"))).toBe("delete");
  });

  it("writes hints the way each platform does", () => {
    expect(hint("download", true)).toBe("⌘D");
    expect(hint("download", false)).toBe("Ctrl+D");
    expect(hint("newFolder", true)).toBe("⌘⇧N");
    expect(hint("newFolder", false)).toBe("Ctrl+Shift+N");
    expect(hint("quickLook", true)).toBe("Space");
    expect(hint("ascend", true)).toBe("⌘↑");
  });

  it("builds a tooltip from the same row that binds the key", () => {
    // The point of the table: a tooltip cannot promise a key the handler is
    // not listening for, because both come from here.
    expect(tooltip("download", true)).toBe("Download (⌘D)");
    expect(tooltip("upload", false)).toBe("Upload (Ctrl+U)");
  });

  it("binds each combination once", () => {
    // Two rows answering the same keystroke would make the earlier one win
    // silently, which is exactly the drift this table exists to prevent.
    const seen = new Set<string>();
    for (const shortcut of SHORTCUTS) {
      for (const key of [shortcut.key, shortcut.alias].filter(Boolean)) {
        const signature = `${key}|${Boolean(shortcut.mod)}|${Boolean(shortcut.shift)}`;
        expect(seen.has(signature), `${signature} is bound twice`).toBe(false);
        seen.add(signature);
      }
    }
  });

  it("gives every verb a label and a reachable hint", () => {
    for (const shortcut of SHORTCUTS) {
      expect(shortcut.label).not.toBe("");
      expect(hint(shortcut.id as ShortcutId, true)).not.toBe("");
    }
  });
});
