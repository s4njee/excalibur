import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defaultBackend, fixtureBackend, writeGuarded } from "./backend";
import type { HttpDownloadOptions, HttpProbe } from "./types";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  });
});

afterEach(() => {
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});

describe("native download contract", () => {
  it("carries mirror and connection choices to one enqueue without a mounted share", async () => {
    invoke.mockResolvedValue("download-1");
    const options: HttpDownloadOptions = {
      connections: 4,
      mirrors: ["https://second.test/file.iso"],
      preferredMirror: 1,
    };
    // A read-only share does not prevent a separate HTTP-to-local download.
    const backend = writeGuarded(defaultBackend(), () => false);
    await expect(backend.enqueueHttpDownload("https://first.test/file.iso", "/tmp/downloads", options))
      .resolves.toBe("download-1");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("enqueue_http_download", {
      url: "https://first.test/file.iso",
      destination: "/tmp/downloads",
      options,
    });
  });

  it("keeps ordinary enqueue compatible and leaves automatic selection to Rust", async () => {
    invoke.mockResolvedValue("download-2");
    await defaultBackend().enqueueHttpDownload("https://first.test/file.iso", "/tmp/downloads");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("enqueue_http_download", {
      url: "https://first.test/file.iso",
      destination: "/tmp/downloads",
      options: undefined,
    });
  });

  it("preserves unknown probe facts and does not enqueue while inspecting", async () => {
    const result: HttpProbe = {
      length: null, ranges: null, filename: "file.iso",
      finalUrl: "https://first.test/file.iso", warning: "Server unavailable",
    };
    invoke.mockResolvedValue(result);
    await expect(defaultBackend().probeHttpDownload("https://first.test/file.iso"))
      .resolves.toEqual(result);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("probe_http_download", {
      url: "https://first.test/file.iso",
    });
  });

  it("gets caps from the engine and preserves native refusal details", async () => {
    invoke.mockResolvedValueOnce({ maxConnections: 16, maxPerHost: 4 });
    const backend = defaultBackend();
    await expect(backend.downloadLimits()).resolves.toEqual({ maxConnections: 16, maxPerHost: 4 });
    expect(invoke).toHaveBeenLastCalledWith("download_limits");
    const refusal = { message: "Invalid mirror index", transient: false, surface: "banner" };
    invoke.mockRejectedValueOnce(refusal);
    await expect(backend.enqueueHttpDownload("https://first.test/file.iso", "/tmp/downloads", {
      preferredMirror: 9,
    })).rejects.toBe(refusal);
  });

  it("does not invent remote facts or limits in the browser fixture", async () => {
    const backend = fixtureBackend();
    const result = await backend.probeHttpDownload("https://first.test/file.iso");
    expect(result.length).toBeNull();
    expect(result.ranges).toBeNull();
    expect(result.warning).toContain("desktop app");
    await expect(backend.downloadLimits()).rejects.toThrow("desktop app");
    expect(invoke).not.toHaveBeenCalled();
  });
});
