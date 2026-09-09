// The reference's table in this app's palette (XDL-E2-S2): the columns, the
// type rule's values, redacted origins, row actions, and the virtual window
// that keeps 10,000 rows at the file list's frame rate.

import { describe, expect, it, vi, afterEach } from "vitest";
import { DEFAULT_PANE_SIZE } from "../state/paneSize";
import { render } from "solid-js/web";
import { DownloadsTable, downloadWindow, DOWNLOAD_ROW_HEIGHT } from "./DownloadsTable";
import { Transfers } from "./Transfers";
import type { Transfer, TransferStatus } from "../state/types";

let dispose: (() => void) | undefined;
const host = () => document.body.lastElementChild as HTMLElement;

function row(id: string, status: TransferStatus, overrides: Partial<Transfer> = {}): Transfer {
  return {
    id,
    direction: "down",
    from: { kind: "share", mount: "smb:nas.local:media", path: `remote/${id}` },
    to: { kind: "local", path: `/local/${id}` },
    name: id,
    sourcePath: `remote/${id}`,
    destPath: `/local/${id}`,
    bytesDone: 0,
    bytesTotal: 10,
    rate: null,
    etaSeconds: null,
    status,
    error: null,
    queuedAtMs: 1,
    mount: "smb:nas.local:media",
    ...overrides,
  };
}

function open(transfers: Transfer[]) {
  const handlers = {
    onPause: vi.fn(),
    onCancel: vi.fn(),
    onRetry: vi.fn(),
    onRemove: vi.fn(),
    onRemount: vi.fn(),
  };
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(
    () => (
      <DownloadsTable
        transfers={transfers}
        onPause={handlers.onPause}
        onCancel={handlers.onCancel}
        onRetry={handlers.onRetry}
        onRemove={handlers.onRemove}
        onRemount={handlers.onRemount}
      />
    ),
    root,
  );
  return handlers;
}

afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.innerHTML = "";
});

describe("columns", () => {
  it("draws the reference's set: file, size, progress, speed, ETA, status", () => {
    open([row("a", "queued")]);
    const headers = [...host().querySelectorAll('[role="columnheader"]')].map((cell) =>
      cell.textContent?.trim(),
    );
    expect(headers).toEqual(["File", "Size", "Progress", "Speed", "ETA", "Status"]);
  });

  it("shows a running row's size, speed, ETA, percentage and badge", () => {
    open([
      row("big.iso", "running", {
        bytesDone: 25,
        bytesTotal: 100,
        rate: 1_000_000,
        etaSeconds: 75,
        from: { kind: "http", url: "https://example.test/big.iso" },
      }),
    ]);
    const table = host().textContent ?? "";
    expect(table).toContain("big.iso");
    expect(table).toContain("example.test");
    expect(table).toContain("100 B");
    expect(table).toContain("Downloading");
    expect(table).toContain("25%");
    expect(table).toContain("1 MB/s");
    expect(table).toContain("1m 15s");
  });

  it("draws an indeterminate bar and a growing count while the size is unknown", () => {
    // XDL-E2-S3: not a bar at zero, no percentage, no denominator — the count
    // climbs as bytes land and the motion says it is alive.
    open([row("stream", "running", { bytesTotal: 0, bytesDone: 512 })]);
    const stream = host().querySelector('[data-transfer="stream"]');
    expect(stream).not.toBeNull();
    const bar = stream?.querySelector('[role="progressbar"]');
    expect(bar).not.toBeNull();
    expect(bar?.getAttribute("aria-valuenow")).toBeNull();
    expect(stream?.textContent).toContain("512 B");
    expect(stream?.textContent).not.toContain("%");
  });

  it("keeps a measured bar on rows that name a total", () => {
    open([row("big.iso", "running", { bytesDone: 25, bytesTotal: 100 })]);
    const bar = host()
      .querySelector('[data-transfer="big.iso"]')
      ?.querySelector('[role="progressbar"]');
    expect(bar?.getAttribute("aria-valuenow")).toBe("25");
  });

  it("opens the shared download context menu and redacts URL credentials", () => {
    const copied: string[] = [];
    const handlers = open([row("secret", "paused", {
      direction: null,
      from: { kind: "http", url: "https://user:pw@example.test/file" },
    })]);
    // Re-render with the clipboard hook; the row menu is intentionally opened
    // by the same native contextmenu gesture as file rows.
    dispose?.();
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(() => (
      <DownloadsTable
        transfers={[row("secret", "paused", { direction: null, from: { kind: "http", url: "https://user:pw@example.test/file" } })]}
        onPause={handlers.onPause}
        onCancel={handlers.onCancel}
        onRetry={handlers.onRetry}
        onCopyUrl={(url) => copied.push(url)}
      />
    ), root);
    host().querySelector('[data-transfer="secret"]')?.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }),
    );
    const item = [...host().querySelectorAll('[role="menuitem"]')].find((node) => node.textContent === "Copy URL");
    item?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(copied).toEqual(["https://example.test/file"]);
  });
});

describe("origin hosts", () => {
  it("redacts credentials out of the displayed host", () => {
    open([
      row("secret", "queued", {
        direction: null,
        mount: null,
        from: { kind: "http", url: "https://user:pw@host/f" },
      }),
    ]);
    const table = host().textContent ?? "";
    expect(table).toContain("host");
    expect(table).not.toContain("user");
    expect(table).not.toContain("pw");
  });

  it("names the share's host and This Mac for local origins", () => {
    open([
      row("up", "running", {
        direction: "up",
        from: { kind: "local", path: "/local/up" },
        to: { kind: "share", mount: "smb:nas.local:media", path: "remote/up" },
      }),
    ]);
    expect(host().textContent).toContain("This Mac");
  });
});

describe("badges and actions", () => {
  it("carries the reference's five states plus cancelled", () => {
    open([
      row("a", "running"),
      row("b", "queued"),
      row("c", "paused"),
      row("d", "done"),
      row("e", "failed", { error: "connection lost" }),
      row("f", "cancelled"),
    ]);
    const table = host().textContent ?? "";
    for (const label of ["Downloading", "Queued", "Paused", "Completed", "Failed", "Cancelled"]) {
      expect(table).toContain(label);
    }
    // A failed row keeps its reason on the row, not just in a tooltip.
    expect(table).toContain("connection lost");
  });

  it("pauses and cancels a running row, resumes a paused one, retries a failed one", () => {
    const handlers = open([row("a", "running"), row("b", "paused"), row("c", "failed")]);
    const buttons = [...host().querySelectorAll("button")].map((button) => button.textContent);
    expect(buttons).toContain("Pause");
    expect(buttons).toContain("Cancel");
    expect(buttons).toContain("Resume");
    expect(buttons).toContain("Retry");

    const click = (label: string) =>
      host()
        .querySelectorAll("button")
        .forEach((button) => {
          if (button.textContent === label) {
            button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
          }
        });
    click("Pause");
    expect(handlers.onPause).toHaveBeenCalledWith("a");
    click("Resume");
    expect(handlers.onRetry).toHaveBeenCalledWith("b");
    click("Retry");
    expect(handlers.onRetry).toHaveBeenCalledWith("c");
  });

  it("asks before removing a partial or completed row", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const handlers = open([row("partial", "paused"), row("complete", "done")]);
    for (const button of host().querySelectorAll("button")) {
      if (button.textContent === "Remove") button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(handlers.onRemove).toHaveBeenCalledWith("partial");
    expect(handlers.onRemove).toHaveBeenCalledWith("complete");
    confirm.mockRestore();
  });
});

describe("virtualisation", () => {
  function many(count: number): Transfer[] {
    return Array.from({ length: count }, (_, index) =>
      row(`file-${index}`, index % 3 === 0 ? "paused" : "queued"),
    );
  }

  it("windows the queue with the file list's arithmetic", () => {
    const transfers = many(10_000);
    const first = downloadWindow(transfers, 0, 600);
    expect(first.length).toBeLessThan(40);
    expect(first[0]?.id).toBe("file-0");
    // Scrolled ten viewports down: the window follows, same row height.
    const scrolled = downloadWindow(transfers, DOWNLOAD_ROW_HEIGHT * 100, 600);
    expect(scrolled.length).toBeLessThan(40);
    expect(scrolled[0]?.id).toBe("file-92");
  });

  it("renders a bounded slice of a 10,000-row queue", () => {
    open(many(10_000));
    const rendered = host().querySelectorAll("[data-transfer]").length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(100);
    // The header is outside the window: always drawn, never virtualised.
    // (Six labelled headers; the actions column's cell is intentionally
    // role-less.)
    expect(host().querySelectorAll('[role="columnheader"]').length).toBe(6);
  });

  it("says nothing is transferring for an empty category", () => {
    open([]);
    expect(host().textContent).toContain("Nothing transferring");
  });
});

describe("the sheet's rows", () => {
  it("draws the same indeterminate bar for an unknown size", () => {
    // S3 covers both bars: the sheet keeps its compact rows, and they must
    // not disagree with the table about what counts as knowing.
    const root = document.createElement("div");
    document.body.append(root);
    const stop = render(
      () => (
        <Transfers
        size={DEFAULT_PANE_SIZE}
        onResize={() => undefined}
        onResizeEnd={() => undefined}
        categories={[{ id: "all", label: "All", count: 0 }]}
        category="all"
        onCategory={() => undefined}
        log={[]}
        view="queue"
        onView={() => undefined}
        verbose={false}
        onVerbose={() => undefined}
        onClearLog={() => undefined}
        onLogAction={() => undefined}
          transfers={[row("stream", "running", { bytesTotal: 0, bytesDone: 512 })]}
          onPause={() => undefined}
          onCancel={() => undefined}
          onRetry={() => undefined}
          onPauseAll={() => undefined}
          onClearFinished={() => undefined}
          onRetryFailed={() => undefined}
          onClearFailed={() => undefined}
          onClose={() => undefined}
        />
      ),
      root,
    );
    const bar = root.querySelector('[data-transfer="stream"] [role="progressbar"]');
    expect(bar).not.toBeNull();
    expect(bar?.getAttribute("aria-valuenow")).toBeNull();
    stop();
  });
});

describe("the footer (XDL-E2-S4)", () => {
  function open(transfers: Transfer[]) {
    const root = document.createElement("div");
    document.body.append(root);
    const stop = render(
      () => (
        <Transfers
        size={DEFAULT_PANE_SIZE}
        onResize={() => undefined}
        onResizeEnd={() => undefined}
        categories={[{ id: "all", label: "All", count: 0 }]}
        category="all"
        onCategory={() => undefined}
        log={[]}
        view="queue"
        onView={() => undefined}
        verbose={false}
        onVerbose={() => undefined}
        onClearLog={() => undefined}
        onLogAction={() => undefined}
          transfers={transfers}
          onPause={() => undefined}
          onCancel={() => undefined}
          onRetry={() => undefined}
          onPauseAll={() => undefined}
          onClearFinished={() => undefined}
          onRetryFailed={() => undefined}
          onClearFailed={() => undefined}
          onClose={() => undefined}
        />
      ),
      root,
    );
    return { root, stop };
  }

  it("shows the active count, remaining bytes and aggregate rate", () => {
    const { root, stop } = open([
      row("a", "running", { bytesDone: 40, bytesTotal: 100, rate: 1_000_000 }),
      row("b", "paused", { rate: 9_000_000 }),
    ]);
    const footer = root.textContent ?? "";
    expect(footer).toContain("1 active");
    expect(footer).toContain("60 B remaining");
    expect(footer).toContain("1 MB/s aggregate");
    // The paused row's made-up 9 MB/s is not part of the total.
    expect(footer).not.toContain("9 MB/s");
    stop();
  });

  it("draws the decorative sparkline next to the numbers", () => {
    const { root, stop } = open([
      row("a", "running", { rate: 1_000_000 }),
      row("b", "running", { rate: 2_000_000 }),
    ]);
    // One real-time sample has been taken (the timer runs in the test too);
    // the sparkline is present and is aria-hidden decoration.
    const svg = root.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    stop();
  });
});
