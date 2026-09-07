// More than one browser window (E23-S1).
//
// Two windows are two `App`s over one engine, so what has to be true here is
// what a single window could never have shown: that a window opens on the mount
// and folder it was *told*, that its chrome names that share and not another
// one's, and that a change made elsewhere on its share reaches it while a
// change made on a different share does not.
//
// The engine half of the story — one session for two windows, closed when the
// last of them leaves — is `crates/engine/tests/session.rs`.

import { describe, expect, it, afterEach, beforeEach, vi } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

/** Two saved shares, neither of them flagged mounted on disk (E1-S6). */
const saved: Bookmark[] = [
  {
    id: "smb:nas.local:media",
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
  {
    id: "sftp:orion.local:backups",
    name: "backups",
    protocol: "sftp",
    host: "orion.local",
    share: "backups",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
];

/**
 * A backend that records what it was asked, and can push a listing change.
 *
 * The fixture answers every listing, which is what makes it usable here: the
 * question is not what the rows are, it is which path was asked for and whose
 * mount the window believes it is on.
 */
function watchable(): Backend & {
  paths: string[];
  changed: (mount: string) => void;
} {
  const inner = fixtureBackend();
  const paths: string[] = [];
  let announce: ((mount: string) => void) | null = null;
  return {
    ...inner,
    async listDir(path: string) {
      paths.push(path);
      return inner.listDir(path);
    },
    onListingChanged(handler: (mount: string) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
    paths,
    changed: (mount: string) => announce?.(mount),
  };
}

function open(
  backend: Backend,
  self: { label: string; mount: string | null; path: string; view: "browse" },
) {
  const saveStore = { load: async () => saved, save: async () => undefined };
  dispose = render(
    () => <App backend={backend} prefs={memoryPrefs()} bookmarks={saveStore} window={self} />,
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
  vi.useRealTimers();
});

describe("a window opened by ⌘N", () => {
  it("opens on the mount and folder it was given", async () => {
    const backend = watchable();
    await open(backend, {
      label: "browser-2",
      mount: "smb:nas.local:media",
      path: "Films/2016",
      view: "browse",
    });

    // The folder, not the share root: ⌘N is for looking at two places in one
    // share, and a second window that always opened at the top would make the
    // user navigate back to where they pressed it.
    expect(backend.paths).toContain("Films/2016");
    expect(backend.paths).not.toContain("");
    // And the chrome names the share the shell put it on. Nothing on disk said
    // so — both bookmarks are stored unmounted, because a `mounted` flag read
    // back from the store is from a previous run.
    //
    // The title bar and the toolbar only: the sidebar lists every bookmark
    // whether or not it is mounted, in every window, which is the point of it.
    const window = host.firstElementChild;
    const chrome = [window?.children[0], window?.children[1]]
      .map((row) => row?.textContent ?? "")
      .join(" ");
    expect(chrome).toContain("media — nas.local");
    expect(chrome).toContain("Films/2016");
    expect(chrome).not.toContain("orion.local");
  });

  it("is the empty state when it was opened on nothing", async () => {
    // The first window, and any window opened while nothing is mounted. The
    // per-window empty state is not a special case: it is what a window with
    // no mount is, whether or not another window has one.
    const backend = fixtureBackend({ disconnected: true });
    await open(backend, { label: "browser-2", mount: null, path: "", view: "browse" });
    expect(host.textContent).toContain("No share connected");
  });
});

describe("two windows on one share", () => {
  it("relists when the other window changes something on its mount", async () => {
    vi.useFakeTimers();
    const backend = watchable();
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={{ load: async () => saved, save: async () => undefined }}
          window={{ label: "browser-2", mount: "smb:nas.local:media", path: "", view: "browse" }}
        />
      ),
      host,
    );
    await vi.advanceTimersByTimeAsync(0);
    const before = backend.paths.length;

    // The other window renamed something. This one is not polling and has no
    // reason of its own to look again.
    backend.changed("smb:nas.local:media");
    await vi.advanceTimersByTimeAsync(0);
    // At once, because the coalescer leads: one change should not wait a
    // quarter second to be shown.
    expect(backend.paths.length).toBe(before + 1);

    // And a burst is one relist, not one each: deleting twenty files in the
    // other window sends twenty of these.
    for (let index = 0; index < 20; index += 1) backend.changed("smb:nas.local:media");
    await vi.advanceTimersByTimeAsync(300);
    expect(backend.paths.length).toBe(before + 2);
  });

  it("ignores a change on a share it is not looking at", async () => {
    vi.useFakeTimers();
    const backend = watchable();
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={{ load: async () => saved, save: async () => undefined }}
          window={{ label: "browser-2", mount: "smb:nas.local:media", path: "", view: "browse" }}
        />
      ),
      host,
    );
    await vi.advanceTimersByTimeAsync(0);
    const before = backend.paths.length;

    // A third window mutating a different mount. The shell addresses these to
    // the windows on that share, so this one should not normally see it at all
    // — but an event in flight when a window switches shares would arrive
    // here, and relisting this share for a change to that one is exactly the
    // cross-talk the id guards against.
    backend.changed("sftp:orion.local:backups");
    await vi.advanceTimersByTimeAsync(300);
    expect(backend.paths.length).toBe(before);
  });
});
