// The New Connection dialog's Local tab, and the chrome a local mount puts in
// the window (E21-S2).
//
// A component test rather than a pure one because the AC is about the DOM: the
// dialog *swaps* the field set rather than greying fields out, so "no host,
// port, username or password" is a claim about what is rendered, and the only
// way it can regress is a field that stays behind on the tab. The rules those
// fields follow are unit-tested in `state/connection.test.ts`.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import { ConnectionDialog } from "./ConnectionDialog";
import { StatusBar } from "./Shell";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import { thisMachine } from "../state/platform";
import type { Bookmark, MountInfo } from "../state/types";

// The virtualised list measures itself on mount; jsdom has no layout.
class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

const tab = (label: string) =>
  [...host.querySelectorAll("button")].find((button) => button.textContent?.trim() === label);

const chooseProtocol = async (label: string) => {
  tab(label)?.click();
  await settle();
};

const field = (id: string) => host.querySelector<HTMLInputElement>(`#${id}`);

/** The label text for a field, which is what the user actually reads. */
const labelFor = (id: string) =>
  host.querySelector<HTMLLabelElement>(`label[for="${id}"]`)?.textContent?.trim();

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

/** The dialog, optionally with a picker, as the Tauri shell supplies one. */
function renderDialog(pickFolder?: () => Promise<string | null>) {
  dispose = render(
    () => (
      <ConnectionDialog
        backend={fixtureBackend()}
        pickFolder={pickFolder}
        onCancel={() => undefined}
        onConnected={() => undefined}
      />
    ),
    host,
  );
}

describe("the dialog's Local tab", () => {
  it("offers a Folder field and no server, port, account or password", async () => {
    renderDialog();
    await chooseProtocol("Local");
    expect(labelFor("share")).toBe("Folder");
    expect(field("share")).not.toBeNull();
    // Not disabled — absent. An inert Server box on this tab would suggest a
    // folder on this machine has one.
    expect(field("host")).toBeNull();
    expect(field("port")).toBeNull();
    expect(field("username")).toBeNull();
    expect(field("password")).toBeNull();
    expect(field("domain")).toBeNull();
    // The Mount-as radio stays: read-only is a choice a folder can honour.
    expect(host.textContent).toContain("Read-only");
  });

  it("previews the folder as a file: URI as it is typed", async () => {
    renderDialog();
    await chooseProtocol("Local");
    const folder = field("share")!;
    folder.value = "/Users/dorian/Documents";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(host.textContent).toContain("file:///Users/dorian/Documents");
  });

  it("refuses to connect a relative path, and says why", async () => {
    renderDialog();
    await chooseProtocol("Local");
    const connect = () => tab("Connect");
    // Nothing typed: the button is dead, as it is on every other tab.
    expect(connect()?.disabled).toBe(true);
    const folder = field("share")!;
    folder.value = "Documents";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(connect()?.disabled).toBe(true);
    expect(connect()?.title).toBe("Folder must be an absolute path.");
    folder.value = "/Users/dorian/Documents";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(connect()?.disabled).toBe(false);
  });

  it("offers no picker where there is none, and keeps the field typeable", async () => {
    // The browser preview has no `tauri-plugin-dialog`. A `Choose…` that did
    // nothing would be worse than no button at all — and the path has to stay
    // typeable either way, because pasting one is faster than clicking to it.
    renderDialog();
    await chooseProtocol("Local");
    expect(tab("Choose…")).toBeUndefined();
    expect(field("share")?.disabled).toBeFalsy();
    expect(field("share")?.readOnly).toBe(false);
  });

  it("takes the picked folder, and leaves the field alone when cancelled", async () => {
    let answer: string | null = "/Volumes/Media";
    renderDialog(async () => answer);
    await chooseProtocol("Local");
    tab("Choose…")?.click();
    await settle();
    expect(field("share")?.value).toBe("/Volumes/Media");
    // Cancel resolves to `null`, and must not clear the path already there.
    answer = null;
    tab("Choose…")?.click();
    await settle();
    expect(field("share")?.value).toBe("/Volumes/Media");
  });

  it("drops the server when switching to Local, and does not smuggle it back", async () => {
    renderDialog();
    await chooseProtocol("SFTP");
    const server = field("host")!;
    server.value = "vault.example";
    server.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    await chooseProtocol("Local");
    const folder = field("share")!;
    folder.value = "/Users/dorian/Documents";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    // The preview is the whole address, and there is no hostname in it.
    expect(host.textContent).toContain("file:///Users/dorian/Documents");
    expect(host.textContent).not.toContain("vault.example");
  });
});

describe("the chrome of a local mount", () => {
  const documents: Bookmark = {
    id: "local::/users/dorian/documents",
    name: "Documents",
    protocol: "local",
    host: "",
    share: "/Users/dorian/Documents",
    mountPreference: "rw",
    mounted: true,
    reachable: true,
  };

  /**
   * The window with the folder actually mounted.
   *
   * Double-clicking the sidebar row rather than seeding `mounted: true`:
   * sessions are not persisted, so the store's flag is cleared on load
   * (E1-S6), and mounting is the only thing that sets it. That also exercises
   * the path a user takes.
   */
  async function renderWindow() {
    // The preview's `mount` refuses on purpose — there is no engine behind a
    // browser — so this is the one thing the fixture has to be lent for the
    // window to hold a share at all. Everything else, the listing included,
    // stays the committed fixture.
    const backend = { ...fixtureBackend(), mount: async () => undefined };
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={{ load: async () => [documents], save: async () => undefined }}
        />
      ),
      host,
    );
    await settle();
    const row = [...host.querySelectorAll('[role="button"]')].find((candidate) =>
      candidate.textContent?.includes("/Users/dorian/Documents"),
    );
    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();
  }

  /** The two rows that describe the mount: title bar, then toolbar. */
  const chrome = () => {
    const window = host.firstElementChild;
    return [window?.children[0], window?.children[1]]
      .map((row) => row?.textContent ?? "")
      .join(" ");
  };

  it("titles the window after the folder and the machine you are sitting at", async () => {
    await renderWindow();
    // `Documents — ` and nothing after it is what the network shape produces
    // for a mount whose host is empty, which is the bug this is here to catch.
    expect(chrome()).toContain(`Documents — ${thisMachine()}`);
  });

  it("reads the breadcrumb as a path, with no authority between the slashes", async () => {
    await renderWindow();
    expect(chrome()).toContain("file://");
    expect(chrome()).toContain("Users");
    expect(chrome()).toContain("Documents");
    // `file://This Mac/Users/…` is what putting the title bar's answer in the
    // path field would give, and it is not a path anyone could type back.
    expect(chrome()).not.toContain(`file://${thisMachine()}`);
  });

  it("makes every crumb of the mount root a way back to it", async () => {
    await renderWindow();
    const crumbs = [...host.querySelectorAll("button")].filter((button) =>
      ["Users", "dorian"].includes(button.textContent?.trim() ?? ""),
    );
    // The host button is gone, so these are the only crumbs naming the mount;
    // they must still be clickable rather than inert text.
    expect(crumbs.length).toBe(2);
    for (const crumb of crumbs) expect(crumb.disabled).toBe(false);
  });
});

describe("the status bar of a local mount", () => {
  const mount = (over: Partial<MountInfo> = {}): MountInfo => ({
    status: "ready",
    writable: true,
    reason: "",
    protocolLabel: "local",
    securityLabel: "local",
    options: "",
    capacity: null,
    caps: {
      randomAccess: true,
      resumeWrite: true,
      atomicRename: true,
      symlinks: true,
      watch: false,
    },
    ...over,
  });

  const renderBar = (info: MountInfo) => {
    dispose = render(
      () => (
        <StatusBar mount={info} path="/Users/dorian/Documents" items={13} selected={0} selectedBytes={0} />
      ),
      host,
    );
  };

  it("says `local` once where a network mount says two things", () => {
    renderBar(mount());
    const status = host.querySelector('[role="status"]')?.textContent ?? "";
    expect(status).toContain("local");
    expect(status).not.toContain("local · local");
  });

  it("still says both where they differ", () => {
    renderBar(mount({ protocolLabel: "xsync v3", securityLabel: "ssh" }));
    expect(host.querySelector('[role="status"]')?.textContent).toContain("xsync v3 · ssh");
  });
});
