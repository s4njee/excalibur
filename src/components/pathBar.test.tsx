// Typing a location into the path field (E1-S2).
//
// The field is breadcrumbs until you click its empty space; then it is an
// input holding the URI you are already at. Three things can be typed and they
// mean three different things — a path inside this share, a `file://` folder on
// this machine, and a share that has to be mounted first.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark } from "../state/types";

// The file list assumes a ResizeObserver; jsdom has none.
class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const saved: Bookmark[] = [];
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

async function open(options: { backend?: Backend; saved?: Bookmark[] } = {}) {
  dispose = render(
    () => (
      <App
        backend={options.backend ?? fixtureBackend()}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => options.saved ?? saved, save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const field = () => host.querySelector<HTMLElement>('[title="Click to type a path or URI"]');
const input = () => host.querySelector<HTMLInputElement>('input[aria-label="Location"]');

/** Click the field's own empty space, which is what opens the editor. */
function startEditing() {
  const box = field();
  expect(box).not.toBeNull();
  box?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

async function type(text: string) {
  startEditing();
  await settle();
  const box = input();
  expect(box).not.toBeNull();
  box!.value = text;
  box!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await settle();
  await settle();
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the path field's editor", () => {
  it("opens on the URI you are already at", async () => {
    await open();
    startEditing();
    await settle();
    // Not empty: the common case is editing the tail of where you are, and an
    // empty box makes you retype the host to do it.
    expect(input()?.value).toMatch(/^[a-z]+:\/\//);
  });

  it("does not open when a breadcrumb was clicked", async () => {
    // The crumbs have to stay clickable; turning a crumb click into an edit
    // would make the field's main job impossible.
    await open();
    const crumb = field()?.querySelector("button");
    crumb?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(input()).toBeNull();
  });

  it("abandons the edit on Escape rather than going somewhere", async () => {
    await open();
    startEditing();
    await settle();
    const box = input()!;
    box.value = "xsync://elsewhere/media";
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(input()).toBeNull();
    expect(field()).not.toBeNull();
  });

  it("refuses a scheme the app does not speak, and says so", async () => {
    await open();
    await type("ftp://nas.local/pub");
    // A banner, not a directory called `ftp:` — which is what treating it as a
    // path would have produced.
    expect(host.textContent).toContain("is not a protocol this app speaks");
  });

  it("closes the editor once something has been typed", async () => {
    await open();
    await type("Films");
    expect(input()).toBeNull();
  });
});

describe("a typed URI for a share the keychain can open", () => {
  const dialog = () => host.querySelector('[aria-label="New Connection"]');

  it("connects instead of asking, when the mount succeeds", async () => {
    // The shell reads the credential by `spec.id`, and `bookmarkId` is
    // `protocol:host:share` — so a URI typed for a saved share finds the
    // stored secret without the dialog being involved at all.
    const mounted: unknown[] = [];
    const backend = {
      ...fixtureBackend(),
      async mount(spec: unknown) {
        mounted.push(spec);
      },
    } as unknown as Backend;
    await open({ backend });
    await type("xsync://nas.local/media");
    expect(mounted).toHaveLength(1);
    expect(mounted[0]).toMatchObject({ protocol: "xsync", host: "nas.local", share: "media" });
    expect(dialog()).toBeNull();
  });

  it("falls back to the prefilled dialog when the mount is refused", async () => {
    // Nothing in the keychain, or the wrong thing: the conversation that can
    // fix it — a password, an unknown host key — lives in the dialog.
    const backend = {
      ...fixtureBackend(),
      async mount() {
        throw new Error("auth failed");
      },
    } as unknown as Backend;
    await open({ backend });
    await type("xsync://nas.local/media");
    expect(dialog()).not.toBeNull();
  });

  it("asks rather than guessing when a non-default port was typed", async () => {
    // `Bookmark` has no port field, so this route would drop it and connect on
    // 22. Connecting to the wrong port silently is worse than asking.
    const mounted: unknown[] = [];
    const backend = {
      ...fixtureBackend(),
      async mount(spec: unknown) {
        mounted.push(spec);
      },
    } as unknown as Backend;
    await open({ backend });
    await type("sftp://nas.local:2222/media");
    expect(mounted).toHaveLength(0);
    expect(dialog()).not.toBeNull();
  });

  it("connects straight away for a port that is the protocol's default", async () => {
    const mounted: unknown[] = [];
    const backend = {
      ...fixtureBackend(),
      async mount(spec: unknown) {
        mounted.push(spec);
      },
    } as unknown as Backend;
    await open({ backend });
    await type("sftp://nas.local:22/media");
    expect(mounted).toHaveLength(1);
  });
});

describe("a URI naming a host that is already bookmarked", () => {
  const dialog = () => host.querySelector('[aria-label="New Connection"]');
  const bookmark: Bookmark = {
    id: "xsync:freya.local:/mnt/data",
    name: "data on freya.local",
    protocol: "xsync",
    host: "freya.local",
    share: "/mnt/data",
    mountPreference: "rw",
    mounted: false,
    reachable: true,
  };

  function mountSpy() {
    const mounted: { id?: string }[] = [];
    const backend = {
      ...fixtureBackend(),
      async mount(spec: { id?: string }) {
        mounted.push(spec);
      },
    } as unknown as Backend;
    return { mounted, backend };
  }

  it("connects with the saved bookmark when only the host is typed", async () => {
    // The reported case: `xsync://freya.local` showed the dialog even though a
    // bookmark for that host connects on its own.
    const { mounted, backend } = mountSpy();
    await open({ backend, saved: [bookmark] });
    await type("xsync://freya.local");
    expect(dialog()).toBeNull();
    // The saved bookmark's own id, which is what the credential is filed
    // under — a fresh one would have a different id and miss the keychain.
    expect(mounted[0]?.id).toBe("xsync:freya.local:/mnt/data");
  });

  it("connects when the share is typed without its leading slash", async () => {
    // A URI cannot carry it: `previewUri` prints `/mnt/data` as `…/mnt/data`.
    const { mounted, backend } = mountSpy();
    await open({ backend, saved: [bookmark] });
    await type("xsync://freya.local/mnt/data");
    expect(dialog()).toBeNull();
    expect(mounted[0]?.id).toBe("xsync:freya.local:/mnt/data");
  });

  it("still asks when the host has two saved shares", async () => {
    const { mounted, backend } = mountSpy();
    await open({
      backend,
      saved: [bookmark, { ...bookmark, id: "xsync:freya.local:/mnt/media", share: "/mnt/media" }],
    });
    await type("xsync://freya.local");
    // Ambiguous. Opening one of them silently would be a guess.
    expect(mounted).toHaveLength(0);
    expect(dialog()).not.toBeNull();
  });
});
