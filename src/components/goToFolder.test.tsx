// The Go to Folder sheet's wiring (E26-S2).
//
// `state/goto.ts` holds the path rules and proves them. What is left here is
// what the sheet does with them, and it has its own ways of being wrong:
// completing from the wrong folder, asking the engine nothing before claiming a
// path is fine, swallowing the error when it is not, or letting Escape close
// the sheet *and* clear the window's filter behind it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import { GoToFolder } from "./GoToFolder";
import type { Entry, Listing, MountInfo } from "../state/types";

const mount: MountInfo = {
  status: "ready",
  writable: true,
  reason: "",
  protocolLabel: "SMB",
  securityLabel: "signed",
  options: "",
  capacity: null,
  caps: {
    randomAccess: true,
    resumeWrite: true,
    atomicRename: true,
    symlinks: true,
    watch: false,
  },
};

const entry = (path: string, kind: Entry["kind"]): Entry => ({
  name: path.slice(path.lastIndexOf("/") + 1),
  path,
  kind,
  size: 0,
  modifiedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: kind === "dir" ? "Folder" : "Document",
  kindCategory: kind === "dir" ? "folder" : "other",
  hidden: false,
});

/** A share deep enough that completing at depth is a different question. */
const TREE: Record<string, Entry[]> = {
  "": [
    entry("docs", "dir"),
    entry("documents", "dir"),
    entry("downloads", "dir"),
    entry("Media", "dir"),
    entry("notes.txt", "file"),
  ],
  docs: [entry("docs/reports", "dir"), entry("docs/report.pdf", "file")],
  "docs/reports": [],
};

let listed: string[] = [];
const listDir = async (path: string): Promise<Listing> => {
  listed.push(path);
  const entries = TREE[path];
  if (!entries) throw new Error(`no such file or directory: ${path}`);
  return { entries, mount };
};

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const field = () => host.querySelector("input") as HTMLInputElement;
const button = (label: string) =>
  [...host.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === label,
  ) as HTMLButtonElement | undefined;
const text = () => host.textContent ?? "";

/** Type into the field the way a user does: replace the whole line. */
async function typePath(value: string) {
  const input = field();
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

function press(key: string, options: { caretAtEnd?: boolean } = {}) {
  const input = field();
  if (options.caretAtEnd) input.setSelectionRange(input.value.length, input.value.length);
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  input.dispatchEvent(event);
  return event;
}

async function show(options: {
  initial?: string;
  onGo?: (path: string) => void;
  onCancel?: () => void;
}) {
  dispose = render(
    () => (
      <GoToFolder
        initial={options.initial ?? ""}
        shareName="media"
        listDir={listDir}
        onGo={options.onGo ?? (() => undefined)}
        onCancel={options.onCancel ?? (() => undefined)}
      />
    ),
    host,
  );
  await settle();
}

beforeEach(() => {
  listed = [];
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the field", () => {
  it("opens on the folder the window is showing", async () => {
    // With a trailing slash, so the first Tab completes a child rather than
    // re-completing the folder the user is already in.
    await show({ initial: "docs" });
    expect(field().value).toBe("/docs/");
  });

  it("opens at the root when that is where the window is", async () => {
    await show({});
    expect(field().value).toBe("/");
  });
});

describe("completion", () => {
  it("extends as far as the folder names agree", async () => {
    await show({});
    await typePath("/d");
    press("Tab");
    await settle();
    // docs, documents and downloads all begin `do`; picking one would be a
    // guess, and this is what a shell does instead.
    expect(field().value).toBe("/do");
    expect(text()).toContain("docs");
    expect(text()).toContain("downloads");
  });

  it("finishes a unique folder and adds the separator", async () => {
    await show({});
    await typePath("/docu");
    press("Tab");
    await settle();
    expect(field().value).toBe("/documents/");
  });

  it("completes from the folder being typed into, not the one on screen", async () => {
    // `docs/rep` must offer `docs`'s names. Completing from the window's own
    // listing is the bug this test exists for.
    await show({});
    await typePath("/docs/rep");
    await settle();
    press("Tab");
    await settle();
    expect(field().value).toBe("/docs/reports/");
    expect(listed).toContain("docs");
  });

  it("never completes to a file", async () => {
    // `docs` holds `reports` and `report.pdf`. A file cannot be gone to, so
    // only the folder is a candidate and the completion is unique.
    await show({});
    await typePath("/docs/report");
    await settle();
    press("Tab");
    await settle();
    expect(field().value).toBe("/docs/reports/");
  });

  it("takes → only from the end of the line", async () => {
    await show({});
    await typePath("/docu");
    // Caret in the middle: → is how the user moves through what they typed.
    field().setSelectionRange(2, 2);
    const inside = press("ArrowRight");
    await settle();
    expect(field().value).toBe("/docu");
    expect(inside.defaultPrevented).toBe(false);

    const atEnd = press("ArrowRight", { caretAtEnd: true });
    await settle();
    expect(field().value).toBe("/documents/");
    expect(atEnd.defaultPrevented).toBe(true);
  });

  it("does nothing, quietly, when nothing matches", async () => {
    await show({});
    await typePath("/zzz");
    press("Tab");
    await settle();
    expect(field().value).toBe("/zzz");
  });
});

describe("going there", () => {
  it("navigates to the resolved path", async () => {
    const went = vi.fn();
    await show({ onGo: went });
    await typePath("/docs/reports/");
    press("Enter");
    await settle();
    // Share-relative, with the slashes the user typed taken off.
    expect(went).toHaveBeenCalledWith("docs/reports");
  });

  it("has a button that means the same as Enter", async () => {
    const went = vi.fn();
    await show({ onGo: went });
    await typePath("docs");
    button("Go")?.click();
    await settle();
    expect(went).toHaveBeenCalledWith("docs");
  });

  it("goes to the share root for a lone slash", async () => {
    const went = vi.fn();
    await show({ initial: "docs", onGo: went });
    await typePath("/");
    press("Enter");
    await settle();
    expect(went).toHaveBeenCalledWith("");
  });

  it("shows the engine's own words for a path that is not there", async () => {
    const went = vi.fn();
    await show({ onGo: went });
    await typePath("/nowhere");
    press("Enter");
    await settle();
    expect(went).not.toHaveBeenCalled();
    expect(text()).toContain("no such file or directory: nowhere");
    // It asked rather than deciding: the message is an answer, not a guess.
    expect(listed).toContain("nowhere");
  });

  it("drops a stale refusal as soon as the path changes", async () => {
    await show({});
    await typePath("/nowhere");
    press("Enter");
    await settle();
    expect(text()).toContain("no such file or directory");
    await typePath("/docs");
    expect(text()).not.toContain("no such file or directory");
  });
});

describe("escape", () => {
  it("cancels, and does not reach the window behind it", async () => {
    const cancelled = vi.fn();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    try {
      await show({ onCancel: cancelled });
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(cancelled).toHaveBeenCalledTimes(1);
      expect(behind).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", behind);
    }
  });

  it("stops listening once it is gone", async () => {
    const cancelled = vi.fn();
    await show({ onCancel: cancelled });
    dispose?.();
    dispose = undefined;
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(cancelled).not.toHaveBeenCalled();
  });
});
