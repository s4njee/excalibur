import { describe, expect, it } from "vitest";
import {
  bookmarkId,
  bookmarkSubtitle,
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

describe("a local bookmark (E21-S2)", () => {
  const local = (over: Partial<Draft> = {}): Draft => ({
    ...emptyDraft("local"),
    share: "/Users/dorian/Documents",
    ...over,
  });

  it("derives its id from the path, not a host", () => {
    expect(bookmarkId(local())).toBe("local::/users/dorian/documents");
    // Two folders are two bookmarks; the same folder twice is one row.
    expect(bookmarkId(local())).toBe(bookmarkId(local()));
    expect(bookmarkId(local())).not.toBe(bookmarkId(local({ share: "/Users/dorian/Music" })));
  });

  it("cannot collide with a network bookmark through its empty host", () => {
    // The protocol prefix is what namespaces it: nothing that reaches a server
    // can produce a `local:` id, and `validate` requires those to have a host.
    const sameShape = { ...emptyDraft("sftp"), host: "", share: "/Users/dorian/Documents" };
    expect(bookmarkId(local())).not.toBe(bookmarkId(sameShape));
  });

  it("lower-cases the path, which is wrong for a path and right for an id", () => {
    // Accepted cost: on a case-sensitive filesystem `/data/Foo` and
    // `/data/foo` would share a row. The bookmark still carries the path
    // exactly as typed, so the folder that opens is never the wrong one.
    expect(bookmarkId(local({ share: "/Data/FOO" }))).toBe(bookmarkId(local({ share: "/data/foo" })));
    expect(fromDraft(local({ share: "/Data/FOO" })).share).toBe("/Data/FOO");
  });

  it("is never probed, so its row is never dimmed", () => {
    // Reachability is not a question a folder answers. `null` means do not
    // probe — a TCP connect to an empty host would fail and dim a row that is
    // perfectly fine.
    expect(probePort(fromDraft(local()))).toBeNull();
    expect(fromDraft(local()).reachable).toBe(true);
  });

  it("says where the folder is instead of naming a host it has not got", () => {
    // `LOCAL · ` — the network shape with an empty host — reads like a row
    // that failed to load.
    expect(bookmarkSubtitle(fromDraft(local()))).toBe("LOCAL · /Users/dorian/Documents");
    expect(bookmarkSubtitle(fromDraft(draft({ protocol: "nfs" })))).toBe("NFS · nas.local");
  });

  it("round-trips through the dialog with no host to lose", () => {
    const reopened = toDraft(fromDraft(local()));
    expect(reopened.protocol).toBe("local");
    expect(reopened.share).toBe("/Users/dorian/Documents");
    expect(reopened.host).toBe("");
  });
});
