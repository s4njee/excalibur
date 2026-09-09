// The icon view (E26-S4).
//
// A grid of cells, not rows. The design draws no such layout — asked for
// directly, 2026-09-09 — so what it must not lose is what the row layouts
// already guarantee: a bounded number of elements for a huge listing, and a
// selection that works the same way.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { FileList } from "./FileList";
import { DEFAULT_WIDTHS, ROW_HEIGHTS } from "../state/columns";
import type { Entry } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const entry = (name: string, kind: Entry["kind"] = "file"): Entry => ({
  name,
  path: name,
  kind,
  size: 10,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: kind === "dir" ? "Folder" : "Text",
  kindCategory: kind === "dir" ? "folder" : "text",
  hidden: false,
});

let banded: { paths: string[]; additive: boolean }[] = [];

function open(entries: Entry[], selected: Set<string> = new Set()) {
  banded = [];
  dispose = render(
    () => (
      <FileList
        layout="icons"
        entries={entries}
        selected={() => selected}
        onSelect={() => undefined}
        onBand={(paths, additive) => banded.push({ paths, additive })}
        reveal={null}
        widths={DEFAULT_WIDTHS}
        onResize={() => undefined}
        onResizeEnd={() => undefined}
        sort={{ column: "name", ascending: true }}
        onSort={() => undefined}
        onOpen={() => undefined}
      />
    ),
    host,
  );
}

const cells = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];

// jsdom lays nothing out, so the list measures a width of zero and the grid
// degenerates to one cell across — correct, but it makes every assertion about
// wrapping vacuous. 1000px is a real window's list.
const LIST_WIDTH = 1000;

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get: () => LIST_WIDTH,
  });
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the icon view", () => {
  it("draws a cell per entry, naming each one", () => {
    open([entry("Arrival.mkv"), entry("Films", "dir")]);
    expect(cells()).toHaveLength(2);
    expect(host.textContent).toContain("Arrival.mkv");
    expect(host.textContent).toContain("Films");
  });

  it("lays the cells out as a grid, not a column", () => {
    open([entry("a"), entry("b"), entry("c")]);
    const grid = cells()[0]?.parentElement;
    // Nine across at 1000px, which is the width divided by a cell.
    expect(grid?.style.gridTemplateColumns).toBe("repeat(9, minmax(0, 1fr))");
  });

  it("renders a bounded slice of a huge listing", () => {
    // The property the whole virtual window exists for. jsdom reports a zero
    // viewport, so this is the overscan alone — which is exactly the bound.
    open(Array.from({ length: 10_000 }, (_, index) => entry(`file-${index}.txt`)));
    expect(cells().length).toBeGreaterThan(0);
    expect(cells().length).toBeLessThan(200);
  });

  it("makes the spacer as tall as the grid, not as tall as a column of rows", () => {
    // A grid of 100 cells is far shorter than 100 rows; a spacer sized in rows
    // would scroll to a page of nothing.
    open(Array.from({ length: 100 }, (_, index) => entry(`file-${index}.txt`)));
    const spacer = cells()[0]?.parentElement?.parentElement;
    const height = Number.parseInt(spacer?.style.height ?? "0", 10);
    // 100 cells, 9 across, is 12 grid rows — not 100 of them.
    expect(height).toBe(Math.ceil(100 / 9) * ROW_HEIGHTS.icons);
  });

  it("carries the selection the way every other layout does", () => {
    open([entry("Arrival.mkv"), entry("Other.mkv")], new Set(["Arrival.mkv"]));
    const chosen = cells().find((cell) => cell.dataset.path === "Arrival.mkv");
    expect(chosen?.getAttribute("aria-selected")).toBe("true");
  });
});

describe("the rubber band over the grid", () => {
  function drag(from: [number, number], to: [number, number], additive = false) {
    // The scrolling rowgroup is where the gesture lives.
    const target = host.querySelector<HTMLElement>("[role='rowgroup']")!;
    target.dispatchEvent(
      new MouseEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: from[0],
        clientY: from[1],
        metaKey: additive,
      }) as unknown as PointerEvent,
    );
    window.dispatchEvent(
      new MouseEvent("pointermove", {
        clientX: to[0],
        clientY: to[1],
      }) as unknown as PointerEvent,
    );
    window.dispatchEvent(new MouseEvent("pointerup") as unknown as PointerEvent);
  }

  it("selects a rectangle of cells, not a run of them", () => {
    // Nine across at 1000px, so a cell is ~111px. A band over the second and
    // third columns of the first two rows covers four cells that are *not*
    // consecutive in index order — which is the whole reason `rowsInBand`
    // could not answer this.
    open(Array.from({ length: 27 }, (_, index) => entry(`file-${index}.txt`)));
    drag([130, 10], [330, 150]);
    const last = banded.at(-1);
    expect(last).toBeDefined();
    expect(last?.paths.length).toBeGreaterThan(0);
    // Two rows deep: the selection spans a gap in index order.
    const indices = (last?.paths ?? []).map((path) =>
      Number.parseInt(path.replace(/\D/g, ""), 10),
    );
    expect(Math.max(...indices) - Math.min(...indices)).toBeGreaterThanOrEqual(9);
  });

  it("carries the additive modifier through", () => {
    open(Array.from({ length: 27 }, (_, index) => entry(`file-${index}.txt`)));
    drag([130, 10], [330, 150], true);
    expect(banded.at(-1)?.additive).toBe(true);
  });

  it("stays a click when the pointer barely moves", () => {
    // The same 4px of slop the row layouts use, in two directions.
    open(Array.from({ length: 27 }, (_, index) => entry(`file-${index}.txt`)));
    drag([130, 10], [132, 12]);
    expect(banded).toHaveLength(0);
  });
});
