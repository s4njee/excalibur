import { describe, expect, it } from "vitest";
import {
  bookmarkId,
  fromDraft,
  probePort,
  remove,
  restore,
  toDraft,
  upsert,
  withMounted,
} from "./bookmarks";
import { emptyDraft, type Draft } from "./connection";
import type { Bookmark } from "./types";

function draft(over: Partial<Draft> = {}): Draft {
  return { ...emptyDraft("xsync"), host: "nas.local", share: "/srv/media", ...over };
}

describe("the bookmark store", () => {
  it("never carries a secret", () => {
    // The whole file is plain JSON in the app's data directory, so this is the
    // property that matters most.
    const bookmark = fromDraft(draft({ password: "hunter2", saveCredentials: true }));
    expect(JSON.stringify(bookmark)).not.toContain("hunter2");
    expect(Object.keys(bookmark)).not.toContain("password");
  });

  it("gives the same share the same id, so reconnecting updates its row", () => {
    expect(bookmarkId(draft())).toBe(bookmarkId(draft({ username: "someone-else" })));
    expect(bookmarkId(draft())).not.toBe(bookmarkId(draft({ share: "/srv/other" })));
    // Case and host are part of identity, but not case-sensitively.
    expect(bookmarkId(draft({ host: "NAS.local" }))).toBe(bookmarkId(draft()));
  });

  it("replaces rather than duplicating on a second connect", () => {
    const first = fromDraft(draft());
    const renamed = { ...fromDraft(draft()), name: "renamed" };
    const list = upsert(upsert([], first), renamed);
    expect(list).toHaveLength(1);
    expect(list[0]?.name).toBe("renamed");
  });

  it("restores a removed bookmark to where it was", () => {
    // Undo that appends would silently reorder the sidebar, which is the kind
    // of thing that makes undo feel untrustworthy.
    const list = [
      fromDraft(draft({ share: "/a" })),
      fromDraft(draft({ share: "/b" })),
      fromDraft(draft({ share: "/c" })),
    ];
    const target = list[1] as Bookmark;
    const index = list.indexOf(target);
    const without = remove(list, target.id);
    expect(without).toHaveLength(2);
    expect(restore(without, target, index).map((b) => b.share)).toEqual(["/a", "/b", "/c"]);
  });

  it("round-trips through the dialog for editing", () => {
    const bookmark = fromDraft(draft({ username: "dorian", mountPreference: "ro" }));
    const reopened = toDraft(bookmark);
    expect(reopened.host).toBe("nas.local");
    expect(reopened.share).toBe("/srv/media");
    expect(reopened.username).toBe("dorian");
    expect(reopened.mountPreference).toBe("ro");
    // The secret is not in the bookmark, so it cannot come back with it.
    expect(reopened.password).toBe("");
  });

  it("marks which rows are mounted without losing the rest", () => {
    const list = [fromDraft(draft({ share: "/a" })), fromDraft(draft({ share: "/b" }))];
    const marked = withMounted(list, new Set([list[0]!.id]));
    expect(marked[0]?.mounted).toBe(true);
    expect(marked[1]?.mounted).toBe(false);
    expect(marked[1]?.host).toBe("nas.local");
  });

  it("probes the port each protocol actually listens on", () => {
    const at = (protocol: Draft["protocol"]) => probePort(fromDraft(draft({ protocol })));
    expect(at("smb")).toBe(445);
    expect(at("nfs")).toBe(2049);
    expect(at("sftp")).toBe(22);
    // xsync runs `xs --server` over SSH, so SSH's port is the one that answers.
    expect(at("xsync")).toBe(22);
  });
});
