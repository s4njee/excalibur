import { describe, expect, it } from "vitest";
import {
  defaultPort,
  emptyDraft,
  isImplemented,
  previewUri,
  shareLabel,
  toSpec,
  usernameHint,
  usernameLabel,
  usesPassword,
  validate,
  type Draft,
} from "./connection";

function draft(over: Partial<Draft> = {}): Draft {
  return { ...emptyDraft(), host: "nas.local", share: "media", ...over };
}

describe("the connection draft", () => {
  it("previews the URI the design shows", () => {
    expect(previewUri(draft({ username: "dorian" }))).toBe("smb://dorian@nas.local/media");
  });

  it("updates the preview from a partial draft rather than waiting for a valid one", () => {
    // The design shows the URI updating as fields change, so a half-typed host
    // must still render something.
    expect(previewUri(emptyDraft())).toBe("smb://…/");
    expect(previewUri(draft({ host: "nas", share: "" }))).toBe("smb://nas/");
  });

  it("omits a port that is the protocol's default", () => {
    // sftp://host:22 is noise; a non-default port is information.
    expect(previewUri(draft({ protocol: "sftp", port: "22" }))).toBe("sftp://nas.local/media");
    expect(previewUri(draft({ protocol: "sftp", port: "2222" }))).toBe(
      "sftp://nas.local/media:2222",
    );
  });

  it("names the second field the way each protocol does", () => {
    expect(shareLabel("smb")).toBe("Share");
    expect(shareLabel("nfs")).toBe("Export");
    expect(shareLabel("sftp")).toBe("Path");
  });

  it("does not offer a password where the protocol has none", () => {
    // NFS authenticates with a sec= mode, not a password. An inert field would
    // suggest otherwise.
    expect(usesPassword("nfs")).toBe(false);
    expect(usesPassword("smb")).toBe(true);
    expect(usesPassword("sftp")).toBe(true);
  });

  it("refuses to connect until the required fields are there", () => {
    expect(validate(draft())).toBeNull();
    expect(validate(draft({ host: "" }))).toBe("A server is required.");
    expect(validate(draft({ share: "" }))).toBe("Share is required.");
    expect(validate(draft({ protocol: "nfs", share: "" }))).toBe("Export is required.");
    expect(validate(draft({ port: "70000" }))).toBe("Port must be between 1 and 65535.");
    expect(validate(draft({ port: "2222" }))).toBeNull();
  });

  it("accepts a username with no password, because a key may answer", () => {
    expect(validate(draft({ protocol: "sftp", username: "dorian", password: "" }))).toBeNull();
  });

  it("says which protocols have a backend behind them", () => {
    // Letting a user fill in a form that cannot connect is worse than telling
    // them up front.
    expect(isImplemented("xsync")).toBe(true);
    expect(isImplemented("sftp")).toBe(true);
    expect(isImplemented("nfs")).toBe(true);
    // SMB is decided but not linkable — see plan.md's E4-S1 blocker.
    expect(isImplemented("smb")).toBe(false);
  });

  it("calls NFS's identity field an identity, not a username", () => {
    // AUTH_SYS sends a uid the server chooses to believe. Calling it a
    // username would promise an authentication step that never happens.
    expect(usernameLabel("nfs")).toBe("Identity");
    expect(usernameLabel("smb")).toBe("Username");
    expect(usernameHint("nfs")).not.toBe("");
    expect(usernameHint("sftp")).toBe("");
  });

  it("keeps NFS's identity out of the previewed URI but inside the bookmark", () => {
    // `nfs://501@host/export` is not a form anyone writes, so the preview does
    // not invent it — but the identity is what the mount will claim, so a
    // bookmark that dropped it would reconnect as somebody else.
    const nfs = draft({ protocol: "nfs", host: "vault", share: "/exports/rw", username: "501:20" });
    expect(previewUri(nfs)).not.toContain("501");
    expect(toSpec(nfs, "id-nfs").username).toBe("501:20");

    const sftp = draft({ protocol: "sftp", host: "vault", username: "dorian" });
    expect(previewUri(sftp)).toContain("dorian@");
  });

  it("defaults NFS to its own port", () => {
    expect(defaultPort("nfs")).toBe(2049);
  });

  it("carries the mount preference as a preference, and no secret", () => {
    const spec = toSpec(draft({ username: "dorian", password: "hunter2" }), "id-1");
    expect(spec.mountPreference).toBe("rw");
    expect(spec.username).toBe("dorian");
    // The password goes to the OS keychain, never through the spec.
    expect(JSON.stringify(spec)).not.toContain("hunter2");
    expect(spec.name).toBe("media");
  });
});
