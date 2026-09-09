// The Add Download dialog's wiring (XDL-E3-S1).
//
// The URL rules live in `state/downloads.ts` and are proved there. What is
// left here is what the dialog does with them, and it has its own ways of
// being wrong: starting with an empty box, with a refused URL, or with no
// destination; letting Escape close the dialog *and* clear the window's
// filter behind it; or filling the field from the clipboard without asking.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import { AddDownloadDialog } from "./AddDownloadDialog";
import type { HttpDownloadOptions } from "../state/types";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const urls = () => host.querySelector<HTMLTextAreaElement>("#download-urls")!;
const dest = () => host.querySelector<HTMLInputElement>("#download-dest")!;
const startButton = () =>
  [...host.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === "Start download",
  ) as HTMLButtonElement | undefined;
const text = () => host.textContent ?? "";

/** Type into the URL box the way a user does: replace the whole box. */
async function typeUrls(value: string) {
  const box = urls();
  box.value = value;
  box.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

async function typeDest(value: string) {
  const field = dest();
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

/** The clipboard's answer, or none when there is no clipboard to ask. */
function stubClipboard(answer: string | null) {
  if (answer === null) {
    vi.stubGlobal("navigator", {});
    return;
  }
  vi.stubGlobal("navigator", { clipboard: { readText: async () => answer } });
}

function show(
  options: {
    folder?: string;
    pickFolder?: () => Promise<string | null>;
    backend?: {
      downloadLimits?: () => Promise<{ maxConnections: number; maxPerHost: number }>;
      probeHttpDownload?: (url: string) => Promise<{ length: number | null; ranges: boolean | null; filename: string | null; finalUrl: string | null; warning: string | null }>;
    };
    busy?: boolean;
    onCancel?: () => void;
    onAdd?: (urls: string[], folder: string, options?: HttpDownloadOptions[]) => void;
  } = {},
) {
  stubClipboard(null);
  dispose = render(
    () => (
      <AddDownloadDialog
        folder={options.folder ?? "/Users/dorian/Downloads"}
        backend={options.backend}
        pickFolder={options.pickFolder}
        busy={options.busy}
        onCancel={options.onCancel ?? (() => undefined)}
        onAdd={options.onAdd ?? (() => undefined)}
      />
    ),
    host,
  );
  return settle();
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
  vi.unstubAllGlobals();
});

describe("starting", () => {
  it("starts disabled with an empty box", async () => {
    await show();
    expect(startButton()?.disabled).toBe(true);
  });

  it("starts one URL per line into the destination", async () => {
    const added: { urls: string[]; folder: string }[] = [];
    await show({ onAdd: (found, folder) => added.push({ urls: found, folder }) });
    await typeUrls("https://a.test/one.iso\nhttps://b.test/two.iso\n");
    expect(startButton()?.disabled).toBe(false);
    startButton()?.click();
    await settle();
    expect(added).toEqual([
      {
        urls: ["https://a.test/one.iso", "https://b.test/two.iso"],
        folder: "/Users/dorian/Downloads",
      },
    ]);
  });

  it("refuses a URL the engine would refuse, and says why before Start", async () => {
    const added: string[][] = [];
    await show({ onAdd: (found) => added.push(found) });
    await typeUrls("https://user:pw@host/f");
    expect(startButton()?.disabled).toBe(true);
    expect(text()).toContain("credentials are not supported");
    startButton()?.click();
    await settle();
    expect(added).toEqual([]);
  });

  it("stays disabled with no destination", async () => {
    await show({ folder: "" });
    await typeUrls("https://a.test/one.iso");
    expect(startButton()?.disabled).toBe(true);
    await typeDest("/Users/dorian/Downloads");
    expect(startButton()?.disabled).toBe(false);
  });

  it("stays disabled while a start is in flight", async () => {
    await show({ busy: true });
    await typeUrls("https://a.test/one.iso");
    // Busy renames the button, so it is found by its busy label — and it
    // must stay inert, because a second click while one start is in flight
    // would enqueue the same rows twice.
    const starting = [...host.querySelectorAll("button")].find(
      (candidate) => candidate.textContent?.trim() === "Starting…",
    ) as HTMLButtonElement | undefined;
    expect(starting?.disabled).toBe(true);
    expect(text()).toContain("Starting…");
  });

  it("starts when probing fails", async () => {
    const added = vi.fn();
    await show({
      onAdd: added,
      backend: { probeHttpDownload: async () => { throw new Error("405"); } },
    });
    await typeUrls("https://a.test/one.iso");
    await new Promise((done) => setTimeout(done, 280));
    expect(text()).toContain("Probe failed");
    expect(startButton()?.disabled).toBe(false);
    startButton()?.click();
    await settle();
    expect(added).toHaveBeenCalled();
  });

  it("locks connections to one when the probe reports no ranges", async () => {
    await show({
      backend: {
        downloadLimits: async () => ({ maxConnections: 16, maxPerHost: 4 }),
        probeHttpDownload: async () => ({ length: 10, ranges: false, filename: null, finalUrl: null, warning: null }),
      },
    });
    await typeUrls("https://a.test/one.iso");
    await new Promise((done) => setTimeout(done, 280));
    expect(text()).toContain("using 1 connection");
    const picker = host.querySelector<HTMLInputElement>("#download-connections");
    expect(picker?.disabled).toBe(true);
  });

  it("treats an advisory probe warning as failed without blocking Start", async () => {
    const added = vi.fn();
    await show({
      onAdd: added,
      backend: {
        downloadLimits: async () => ({ maxConnections: 16, maxPerHost: 4 }),
        probeHttpDownload: async () => ({
          length: null,
          ranges: null,
          filename: "one.iso",
          finalUrl: "https://a.test/one.iso",
          warning: "HEAD returned 405",
        }),
      },
    });
    await typeUrls("https://a.test/one.iso");
    await new Promise((done) => setTimeout(done, 280));
    expect(text()).toContain("Probe failed · size unknown · ranges unknown");
    expect(startButton()?.disabled).toBe(false);
    startButton()?.click();
    expect(added).toHaveBeenCalledTimes(1);
  });

  it("bounds a batch probe to one request at a time", async () => {
    let release: (() => void) | undefined;
    const first = new Promise<void>((resolve) => {
      release = resolve;
    });
    const calls: string[] = [];
    await show({
      backend: {
        probeHttpDownload: async (url) => {
          calls.push(url);
          if (calls.length === 1) await first;
          return {
            length: 10,
            ranges: true,
            filename: null,
            finalUrl: null,
            warning: null,
          };
        },
      },
    });
    await typeUrls("https://a.test/one.iso\nhttps://b.test/two.iso");
    await new Promise((done) => setTimeout(done, 280));
    expect(calls).toEqual(["https://a.test/one.iso"]);
    release?.();
    await settle();
    await settle();
    expect(calls).toEqual(["https://a.test/one.iso", "https://b.test/two.iso"]);
  });

  it("keeps per-URL range limits when a separate batch mixes servers", async () => {
    const added = vi.fn();
    await show({
      onAdd: added,
      backend: {
        downloadLimits: async () => ({ maxConnections: 8, maxPerHost: 4 }),
        probeHttpDownload: async (url) => ({
          length: 10,
          ranges: url.includes("b.test"),
          filename: null,
          finalUrl: null,
          warning: null,
        }),
      },
    });
    await typeUrls("https://a.test/one.iso\nhttps://b.test/two.iso");
    await new Promise((done) => setTimeout(done, 300));
    const three = [...host.querySelectorAll<HTMLInputElement>("input[name='download-connections']")]
      .find((input) => input.nextElementSibling?.textContent === "3");
    expect(three?.disabled).toBe(false);
    three?.click();
    startButton()?.click();
    expect(added).toHaveBeenCalledWith(
      ["https://a.test/one.iso", "https://b.test/two.iso"],
      "/Users/dorian/Downloads",
      [{ connections: 1 }, { connections: 3 }],
    );
  });

  it("sends one mirror job and the selected starting source", async () => {
    const added = vi.fn();
    await show({ onAdd: added });
    await typeUrls("https://a.test/one.iso\nhttps://b.test/one.iso\nhttps://c.test/one.iso");
    const modes = host.querySelectorAll<HTMLInputElement>("input[name='download-mode']");
    modes[1]?.click();
    const sources = host.querySelectorAll<HTMLInputElement>("input[name='preferred-mirror']");
    sources[2]?.click();
    startButton()?.click();
    expect(added).toHaveBeenCalledWith(
      ["https://a.test/one.iso", "https://b.test/one.iso", "https://c.test/one.iso"],
      "/Users/dorian/Downloads",
      [{
        mirrors: ["https://b.test/one.iso", "https://c.test/one.iso"],
        preferredMirror: 2,
      }],
    );
  });

  it("takes the picked folder, and leaves the field alone when cancelled", async () => {
    let answer: string | null = "/Volumes/Media";
    await show({ pickFolder: async () => answer });
    const choose = [...host.querySelectorAll("button")].find(
      (candidate) => candidate.textContent?.trim() === "Choose…",
    );
    choose?.click();
    await settle();
    expect(dest().value).toBe("/Volumes/Media");
    answer = null;
    choose?.click();
    await settle();
    expect(dest().value).toBe("/Volumes/Media");
  });

  it("offers no picker where there is none, and keeps the field typeable", async () => {
    // The browser preview has no `tauri-plugin-dialog`; the connection
    // dialog's Local tab sets the same precedent.
    await show();
    expect(
      [...host.querySelectorAll("button")].some(
        (candidate) => candidate.textContent?.trim() === "Choose…",
      ),
    ).toBe(false);
    expect(dest().disabled).toBe(false);
  });

  it("asks the webview to leave the URL and path alone", async () => {
    // Nothing here is prose — a URL and a path — so the platform's help can
    // only corrupt it. Enumerated rather than spot-checked, because the
    // failure is a field added later without the attributes.
    await show({ pickFolder: async () => null });
    for (const field of [urls(), dest()]) {
      expect(field.getAttribute("autocapitalize")).toBe("none");
      expect(field.getAttribute("autocorrect")).toBe("off");
      expect(field.getAttribute("spellcheck")).toBe("false");
      expect(field.getAttribute("autocomplete")).toBeTruthy();
    }
  });
});

describe("the clipboard", () => {
  it("is never read, because reading it costs the user a click", async () => {
    // macOS Sequoia raises a system "Paste" confirmation for a programmatic
    // pasteboard read. The dialog used to read on mount and on every window
    // focus to offer a URL it found, which meant reaching the downloader took
    // two clicks: one for the button, one to grant the paste
    // (reported 2026-09-09). A real Cmd+V raises nothing.
    let reads = 0;
    vi.stubGlobal("navigator", {
      clipboard: {
        readText: async () => {
          reads += 1;
          return "https://cdn.test/big.iso";
        },
      },
    });
    await show();
    window.dispatchEvent(new Event("focus"));
    await settle();
    await settle();
    expect(reads).toBe(0);
  });

  it("leaves the URL field focused, so a paste needs no clicks at all", async () => {
    await show();
    expect(document.activeElement).toBe(urls());
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

  it("lets every other key through", async () => {
    const cancelled = vi.fn();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    try {
      await show({ onCancel: cancelled });
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
      expect(cancelled).not.toHaveBeenCalled();
      expect(behind).toHaveBeenCalledTimes(1);
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

describe("closing it", () => {
  it("closes on Cancel, and on a click outside but not inside", async () => {
    const cancelled = vi.fn();
    await show({ onCancel: cancelled });
    host.querySelector('[role="dialog"]')?.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    expect(cancelled).not.toHaveBeenCalled();
    [...host.querySelectorAll("button")].find(
      (candidate) => candidate.textContent?.trim() === "Cancel",
    )?.click();
    expect(cancelled).toHaveBeenCalledTimes(1);
  });
});
