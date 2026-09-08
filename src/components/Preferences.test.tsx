import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { DEFAULTS, type Settings } from "../state/settings";
import type { HttpDownloadLimits } from "../state/types";
import { Preferences } from "./Preferences";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

function show(
  settings: Settings = DEFAULTS,
  onChange: (key: keyof Settings, value: unknown) => void = () => undefined,
  downloadLimits?: HttpDownloadLimits,
) {
  dispose = render(
    () => (
      <Preferences
        settings={settings}
        onChange={onChange as PreferencesProps["onChange"]}
        onClose={() => undefined}
        downloadLimits={downloadLimits}
      />
    ),
    host,
  );
}

type PreferencesProps = Parameters<typeof Preferences>[0];

describe("Downloads preferences", () => {
  it("shows the provider defaults without daemon controls", () => {
    show();
    expect(host.querySelector("#downloads")).toBeTruthy();
    expect(host.querySelector<HTMLSelectElement>("#download-connections")?.value).toBe("auto");
    expect(host.querySelector<HTMLInputElement>("#download-retry-attempts")?.value).toBe("5");
    expect(host.querySelector<HTMLInputElement>("#download-retry-initial-wait")?.value).toBe("1");
    expect(host.querySelector<HTMLInputElement>("#download-retry-max-wait")?.value).toBe("30");
    expect(host.textContent).not.toContain("RPC");
    expect(host.textContent).not.toContain("Restart engine");
    expect(host.textContent).not.toContain("secret token");
  });

  it("emits a default connection choice and a complete retry policy", async () => {
    const changes: [keyof Settings, unknown][] = [];
    show(DEFAULTS, (key, value) => changes.push([key, value]));

    const connections = host.querySelector<HTMLSelectElement>("#download-connections")!;
    connections.value = "3";
    connections.dispatchEvent(new Event("change", { bubbles: true }));
    const attempts = host.querySelector<HTMLInputElement>("#download-retry-attempts")!;
    attempts.value = "8";
    attempts.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(changes).toEqual([
      ["downloadConnections", 3],
      ["downloadRetry", { ...DEFAULTS.downloadRetry, maxAttempts: 8 }],
    ]);
  });

  it("uses the native per-origin limit for the connection choices", () => {
    show(DEFAULTS, () => undefined, { maxConnections: 16, maxPerHost: 2 });
    const connections = host.querySelector<HTMLSelectElement>("#download-connections")!;
    expect([...connections.options].map((option) => option.value)).toEqual(["auto", "1", "2"]);
  });
});
