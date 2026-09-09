// Column view (E26-S4): one pane per level of the path.
//
// The one layout that is not the current directory drawn differently — it
// draws the ancestors too, each its own listing — so what these check is the
// thing the other layouts never had to: that the columns agree with each other
// and with the path.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ColumnView } from "./ColumnView";
import type { Entry, Listing } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const entry = (path: string, kind: Entry["kind"] = "file"): Entry => ({
  name: path.split("/").at(-1) ?? path,
  path,
  kind,
  size: 1,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: kind === "dir" ? "Folder" : "Text",
  kindCategory: kind === "dir" ? "folder" : "text",
  hidden: path.split("/").at(-1)?.startsWith(".") ?? false,
});

/** A little tree: root has Films and a dotfile; Films has Arrival and a file. */
const TREE: Record<string, Entry[]> = {
  "": [entry("Films", "dir"), entry("Music", "dir"), entry(".hidden")],
  Films: [entry("Films/Arrival", "dir"), entry("Films/notes.txt")],
  "Films/Arrival": [entry("Films/Arrival/arrival.mkv")],
  Music: [entry("Music/album.flac")],
};

function open(
  path: string,
  overrides: {
    prepare?: (entries: Entry[]) => Entry[];
    onNavigate?: (path: string) => void;
    onSelect?: (path: string) => void;
    listDir?: (path: string) => Promise<Listing>;
    selected?: Set<string>;
  } = {},
) {
  const listDir =
    overrides.listDir ??
    (async (target: string) => ({ entries: TREE[target] ?? [] }) as Listing);
  dispose = render(
    () => (
      <ColumnView
        path={path}
        listDir={listDir}
        prepare={overrides.prepare ?? ((entries) => entries)}
        selected={() => overrides.selected ?? new Set()}
        onSelect={(chosen) => overrides.onSelect?.(chosen)}
        onNavigate={(chosen) => overrides.onNavigate?.(chosen)}
        onOpen={() => undefined}
      />
    ),
    host,
  );
}

const panes = () => [...host.querySelectorAll<HTMLElement>('[role="listbox"]')];
const rowsIn = (pane: HTMLElement) => [...pane.querySelectorAll<HTMLElement>("[data-path]")];
const rowFor = (path: string) => host.querySelector<HTMLElement>(`[data-path="${path}"]`);

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("column view", () => {
  it("draws one pane per level, ending at the directory you are in", async () => {
    open("Films/Arrival");
    await settle();
    await settle();
    // Root, Films, Films/Arrival.
    expect(panes()).toHaveLength(3);
    expect(panes()[0]?.getAttribute("aria-label")).toBe("Share root");
    expect(panes()[2]?.getAttribute("aria-label")).toBe("Films/Arrival");
  });

  it("shows one pane at the root, not zero", async () => {
    open("");
    await settle();
    await settle();
    expect(panes()).toHaveLength(1);
    expect(rowsIn(panes()[0]!).map((row) => row.dataset.path)).toContain("Films");
  });

  it("lists each level from its own directory", async () => {
    open("Films/Arrival");
    await settle();
    await settle();
    expect(rowsIn(panes()[1]!).map((row) => row.dataset.path)).toEqual([
      "Films/Arrival",
      "Films/notes.txt",
    ]);
    expect(rowsIn(panes()[2]!).map((row) => row.dataset.path)).toEqual([
      "Films/Arrival/arrival.mkv",
    ]);
  });

  it("marks the child that opened the next pane", async () => {
    // Not selection: it says where the path goes, and both can be true of
    // different rows at once.
    open("Films/Arrival");
    await settle();
    await settle();
    expect(rowFor("Films")?.className).toContain("columnRowOnPath");
    expect(rowFor("Music")?.className).not.toContain("columnRowOnPath");
  });

  it("navigates on a single click of a folder", async () => {
    // The whole gesture of this view: a folder opens a pane rather than
    // needing the double-click every other layout wants.
    const onNavigate = vi.fn();
    open("Films", { onNavigate });
    await settle();
    await settle();
    rowFor("Films/Arrival")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(onNavigate).toHaveBeenCalledWith("Films/Arrival");
  });

  it("selects a file rather than navigating, because nothing is past it", async () => {
    const onNavigate = vi.fn();
    const onSelect = vi.fn();
    open("Films", { onNavigate, onSelect });
    await settle();
    await settle();
    rowFor("Films/notes.txt")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(onSelect).toHaveBeenCalledWith("Films/notes.txt");
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it("obeys the window's own visibility and order rules in every pane", async () => {
    // A pane that showed dotfiles the list was hiding, or sorted differently
    // from the pane beside it, would be describing a different folder.
    open("", { prepare: (entries) => entries.filter((candidate) => !candidate.hidden) });
    await settle();
    await settle();
    expect(rowsIn(panes()[0]!).map((row) => row.dataset.path)).toEqual(["Films", "Music"]);
  });

  it("says what went wrong instead of drawing an empty folder", async () => {
    open("Films", {
      listDir: async (target) => {
        if (target === "Films") throw new Error("stale file handle");
        return { entries: TREE[target] ?? [] } as Listing;
      },
    });
    await settle();
    await settle();
    expect(host.textContent).toContain("stale file handle");
  });

  it("renders a bounded slice of a huge directory", async () => {
    const many = Array.from({ length: 10_000 }, (_, index) => entry(`file-${index}.txt`));
    open("", { listDir: async () => ({ entries: many }) as Listing });
    await settle();
    await settle();
    const drawn = rowsIn(panes()[0]!);
    expect(drawn.length).toBeGreaterThan(0);
    expect(drawn.length).toBeLessThan(200);
  });
});
