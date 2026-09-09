import { describe, expect, it } from "vitest";
import { parseLocation } from "./location";
import { emptyDraft, previewUri } from "./connection";

describe("what the path field was given", () => {
  it("treats a bare path as somewhere in the share already open", () => {
    expect(parseLocation("Films/Arrival 2016")).toEqual({
      kind: "navigate",
      path: "Films/Arrival 2016",
    });
    expect(parseLocation("/media/films")).toEqual({ kind: "navigate", path: "/media/films" });
  });

  it("says nothing about an empty field", () => {
    // Not an error: a cleared field is a cancelled edit, and complaining about
    // it would put a banner up for pressing Escape.
    expect(parseLocation("")).toBeNull();
    expect(parseLocation("   ")).toBeNull();
  });

  it("reads a file:// folder on this machine", () => {
    expect(parseLocation("file:///Users/dorian/Downloads")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("local"), share: "/Users/dorian/Downloads" },
    });
  });

  it("accepts the localhost authority other tools print", () => {
    expect(parseLocation("file://localhost/Users/dorian")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("local"), share: "/Users/dorian" },
    });
  });

  it("expands ~ against the home the window knows", () => {
    expect(parseLocation("file://~/Downloads", "/Users/dorian")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("local"), share: "/Users/dorian/Downloads" },
    });
    expect(parseLocation("file://~", "/Users/dorian")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("local"), share: "/Users/dorian" },
    });
  });

  it("refuses a file:// location that is not absolute", () => {
    // With no home to expand against there is nothing to resolve it to, and
    // guessing the share's root would open a folder the user did not name.
    expect(parseLocation("file://Downloads")).toEqual({
      kind: "invalid",
      reason: "a file:// location needs an absolute path",
    });
  });

  it("reads an xsync share, with and without a user", () => {
    expect(parseLocation("xsync://nas.local/media")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("xsync"), host: "nas.local", share: "media" },
    });
    expect(parseLocation("xsync://dorian@nas.local/media")).toEqual({
      kind: "connect",
      draft: { ...emptyDraft("xsync"), host: "nas.local", username: "dorian", share: "media" },
    });
  });

  it("takes the port from the authority, where a URI puts it", () => {
    expect(parseLocation("sftp://dorian@nas.local:2222/media")).toEqual({
      kind: "connect",
      draft: {
        ...emptyDraft("sftp"),
        host: "nas.local",
        port: "2222",
        username: "dorian",
        share: "media",
      },
    });
  });

  it("takes the last @ so a username may contain one", () => {
    // `nfs://501:20@vault/exports` is the NFS identity shape, and an SMB
    // username can be an email address.
    const parsed = parseLocation("smb://dorian@corp.example@nas.local/media");
    expect(parsed).toMatchObject({
      kind: "connect",
      draft: { username: "dorian@corp.example", host: "nas.local" },
    });
  });

  it("refuses a scheme this app does not speak", () => {
    // Rather than reading it as a directory called `ftp:`, which is what
    // falling through to the path branch would do.
    expect(parseLocation("ftp://nas.local/pub")).toEqual({
      kind: "invalid",
      reason: "ftp:// is not a protocol this app speaks",
    });
  });

  it("refuses a URI with no host, and a port that is not a number", () => {
    expect(parseLocation("xsync:///media")).toMatchObject({ kind: "invalid" });
    expect(parseLocation("sftp://nas.local:ssh/media")).toEqual({
      kind: "invalid",
      reason: "ssh is not a port",
    });
  });

  it("is case-insensitive about the scheme", () => {
    expect(parseLocation("XSYNC://nas.local/media")).toMatchObject({ kind: "connect" });
  });
});

describe("round-tripping what the app itself prints", () => {
  // The field shows `previewUri`'s output and now also reads it. A URI the app
  // printed that it cannot read back is the bug this pair exists to prevent.
  const cases = [
    { ...emptyDraft("xsync"), host: "nas.local", share: "media" },
    { ...emptyDraft("xsync"), host: "nas.local", username: "dorian", share: "media" },
    { ...emptyDraft("sftp"), host: "nas.local", port: "2222", username: "d", share: "media" },
    { ...emptyDraft("smb"), host: "nas.local", username: "dorian", share: "media" },
    { ...emptyDraft("nfs"), host: "vault", share: "exports/rw" },
    { ...emptyDraft("local"), share: "/Users/dorian/Downloads" },
  ];

  for (const draft of cases) {
    it(`reads back ${previewUri(draft)}`, () => {
      const parsed = parseLocation(previewUri(draft));
      expect(parsed).toEqual({ kind: "connect", draft });
    });
  }
});
