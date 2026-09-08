import { afterEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { DownloadDetail } from "./DownloadDetail";
import type { Transfer } from "../state/types";

let dispose: (() => void) | undefined;

function transfer(progress: Transfer["progress"]): Transfer {
  return {
    id: "download",
    direction: null,
    from: { kind: "http", url: "https://example.test/archive.iso" },
    to: { kind: "local", path: "/tmp" },
    name: "archive.iso",
    sourcePath: "https://example.test/archive.iso",
    destPath: "/tmp/archive.iso",
    bytesDone: 3 * 1024 * 1024,
    bytesTotal: 32 * 1024 * 1024,
    rate: 10 * 1024 * 1024,
    etaSeconds: 3,
    status: "running",
    error: null,
    queuedAtMs: 1,
    progress,
  };
}

afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.innerHTML = "";
});

describe("DownloadDetail", () => {
  it("uses each segment's real offset and reports stalled lanes in words", () => {
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(
      () => (
        <DownloadDetail
          transfer={transfer({
            files: [
              {
                path: "archive.iso",
                completed: 3 * 1024 * 1024,
                total: 32 * 1024 * 1024,
                segments: [
                  {
                    stream: 0,
                    offset: 0,
                    length: 16 * 1024 * 1024,
                    written: 2 * 1024 * 1024,
                    rate: 10 * 1024 * 1024,
                    stalled: false,
                  },
                  {
                    stream: 1,
                    offset: 16 * 1024 * 1024,
                    length: 16 * 1024 * 1024,
                    written: 1 * 1024 * 1024,
                    rate: 0,
                    stalled: true,
                  },
                ],
              },
            ],
            retries: 2,
            redirects: 1,
          })}
        />
      ),
      root,
    );

    const lanes = [...root.querySelectorAll("[data-stream]")];
    expect(lanes).toHaveLength(2);
    expect(lanes[1]?.querySelector("span")?.textContent).toContain("Stream 2");
    expect(root.textContent).toContain("Stalled");
    expect(root.textContent).toContain("2 retries");
    expect(root.querySelectorAll("[data-range]")).toHaveLength(2);
    const range = root.querySelector('[data-stream="1"] [data-range]') as HTMLElement;
    expect(range.style.left).toBe("50%");
  });

  it("does not invent range geometry when the total is unknown", () => {
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(
      () => (
        <DownloadDetail
          transfer={transfer({
            files: [
              {
                path: "stream.bin",
                completed: 4,
                total: null,
                segments: [
                  {
                    stream: 0,
                    offset: 4,
                    length: null,
                    written: 4,
                    rate: 1,
                    stalled: false,
                  },
                ],
              },
            ],
            retries: 0,
            redirects: 0,
          })}
        />
      ),
      root,
    );

    expect(root.querySelectorAll("[data-range]")).toHaveLength(0);
    expect(root.textContent).toContain("4 B");
  });
});
