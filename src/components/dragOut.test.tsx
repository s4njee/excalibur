// The gesture that starts a drag out of the list (E24-S2).
//
// `dnd.ts` decides what a drop means, and its tests cover that. What no unit
// test of a pure module can cover is the part that lives in the pointer: a
// drag has to start from the *name* and only after the pointer has actually
// moved. Both halves fail silently in the wild —
//
//   - start it anywhere on the row, and the rubber band E8-S8 reserved the
//     rest of the row for becomes a drag of whatever was under the finger;
//   - start it on `pointerdown` with no threshold, and every click on a
//     filename becomes a drag, which is how a single-click selection turns
//     into a move nobody asked for.
//
// So this drives real pointer events against the rendered list and asks what
// the backend was told.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

// jsdom has no 2d context and logs loudly when asked for one. The drag image
// is not what this test is about — the paths are — and an empty image is what
// the app already falls back to when a context cannot be had.
HTMLCanvasElement.prototype.getContext = (() => null) as never;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
let dragged: { paths: string[]; folders: string[] }[];

const settle = () => new Promise((done) => setTimeout(done, 0));

/** The rows, which carry their share-relative path. */
const rows = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];

/** One row by its share-relative path, which is what the list keys them on. */
const rowFor = (path: string): HTMLElement => {
  const row = rows().find((candidate) => candidate.getAttribute("data-path") === path);
  if (!row) throw new Error(`no row for ${path}`);
  return row;
};

/** The name cell of a row: the handle a drag is meant to start from. */
const nameCell = (row: HTMLElement): HTMLElement => {
  const cell = row.querySelector<HTMLElement>('[role="gridcell"]');
  if (!cell) throw new Error("a row with no cells");
  return cell;
};

/** The size cell, which is the part of a row that bands rather than drags. */
const otherCell = (row: HTMLElement): HTMLElement => {
  const cells = row.querySelectorAll<HTMLElement>('[role="gridcell"]');
  const cell = cells[1];
  if (!cell) throw new Error("a row with only one cell");
  return cell;
};

function press(target: HTMLElement, x: number, y: number) {
  target.dispatchEvent(
    new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: x, clientY: y }),
  );
}

function moveTo(x: number, y: number) {
  window.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, clientX: x, clientY: y }));
}

/** Always, after every press: a press with no release leaks its listeners. */
function release() {
  window.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
}

beforeEach(async () => {
  dragged = [];
  const backend: Backend = {
    ...fixtureBackend(),
    async beginDrag(paths: string[], folders: string[]): Promise<void> {
      dragged.push({ paths, folders });
    },
  };
  host = document.createElement("div");
  document.body.append(host);
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
});

afterEach(() => {
  release();
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("dragging out of the list", () => {
  it("does not start on a press alone", async () => {
    // A click is a selection. If this ever starts a drag, every single click on
    // a filename in the app becomes a potential move.
    const row = rows()[0];
    expect(row).toBeDefined();
    press(nameCell(row as HTMLElement), 100, 100);
    await settle();
    expect(dragged).toEqual([]);
  });

  it("does not start on a press-and-wobble inside the slop", async () => {
    const row = rows()[0] as HTMLElement;
    press(nameCell(row), 100, 100);
    moveTo(102, 101);
    await settle();
    expect(dragged).toEqual([]);
  });

  it("starts once the pointer has moved past the slop, carrying that row", async () => {
    const row = rowFor("Arrival.2016.mkv");
    press(nameCell(row), 100, 100);
    moveTo(120, 100);
    await settle();
    // A file, so nothing is named as a folder — the other half of the payload
    // the promise needs is the empty list, not an absent one.
    expect(dragged).toEqual([{ paths: ["Arrival.2016.mkv"], folders: [] }]);
  });

  it("starts only from the name cell, so the rest of the row still bands", async () => {
    // E8-S8 reserved the name cell for this and gave the rest of the row to
    // rubber-band selection. A drag that started anywhere would take a feature
    // away that already shipped.
    const row = rows()[0] as HTMLElement;
    press(otherCell(row), 400, 100);
    moveTo(420, 140);
    await settle();
    expect(dragged).toEqual([]);
  });

  it("drags the whole selection when the grabbed row is part of it", async () => {
    // Finder's rule. Dragging one row out of a selection of five and having
    // the other four come along is only correct because the user can see they
    // are selected — and dragging a row that is *not* selected must not.
    const [first, second] = rows() as [HTMLElement, HTMLElement];
    first.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    second.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    await settle();

    press(nameCell(second), 100, 130);
    moveTo(130, 130);
    await settle();
    expect(dragged[0]?.paths).toEqual([
      first.getAttribute("data-path"),
      second.getAttribute("data-path"),
    ]);
  });

  it("tells the engine which of the dragged rows are folders", async () => {
    // The half of the payload that only the *listing* knows (E24-S1). A path
    // cannot say it: `Documentaries` and `Dune.Part.Two.mkv` differ by their
    // row's kind, not by their string. Get this wrong and the drag promises a
    // folder as a file, which Finder accepts and then writes an empty file
    // for — a drop that reports success and produces nothing.
    press(nameCell(rowFor("Documentaries")), 100, 200);
    moveTo(130, 200);
    await settle();
    expect(dragged[0]?.paths).toEqual(["Documentaries"]);
    expect(dragged[0]?.folders).toEqual(["Documentaries"]);
  });

  it("drags only the grabbed row when it is not in the selection", async () => {
    const all = rows() as HTMLElement[];
    const [first, , third] = all as [HTMLElement, HTMLElement, HTMLElement];
    first.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    press(nameCell(third), 100, 160);
    moveTo(130, 160);
    await settle();
    expect(dragged[0]?.paths).toEqual([third.getAttribute("data-path")]);
  });
});
