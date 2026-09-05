import { describe, expect, it } from "vitest";
import {
  connectionName,
  defaultPort,
  emptyDraft,
  isImplemented,
  previewUri,
  shareLabel,
  switchProtocol,
  toSpec,
  uriScheme,
  usernameHint,
  usernameLabel,
  usesDomain,
  usesHost,
  usesIdentity,
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
    expect(shareLabel("local")).toBe("Folder");
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

describe("the Local tab (E21-S2)", () => {
  const folder = (over: Partial<Draft> = {}): Draft => ({
    ...emptyDraft("local"),
    share: "/Users/dorian/Documents",
    ...over,
  });

  it("asks for a folder and nothing else", () => {
    // The dialog swaps the field set rather than greying fields out, so this
    // is the whole spec of the tab: one field, plus Mount-as. A server, a
    // port, an account or a password here would each be a box that cannot be
    // filled in usefully.
    expect(shareLabel("local")).toBe("Folder");
    expect(usesHost("local")).toBe(false);
    expect(usesIdentity("local")).toBe(false);
    expect(usesPassword("local")).toBe(false);
    expect(usesDomain("local")).toBe(false);
    expect(defaultPort("local")).toBeNull();
  });

  it("previews the folder as a file: URI, percent-encoding nothing", () => {
    expect(uriScheme("local")).toBe("file");
    expect(previewUri(folder())).toBe("file:///Users/dorian/Documents");
    // A preview, not a URL anything dereferences: the path a user checks
    // against the folder they picked beats one with `%20` in it.
    expect(previewUri(folder({ share: "/Volumes/Big Disk/Raw Footage" }))).toBe(
      "file:///Volumes/Big Disk/Raw Footage",
    );
    // And it moves while the field is still empty, like the others.
    expect(previewUri(emptyDraft("local"))).toBe("file://…");
  });

  it("refuses a missing folder and a relative one", () => {
    expect(validate(folder())).toBeNull();
    expect(validate(folder({ share: "" }))).toBe("Folder is required.");
    expect(validate(folder({ share: "   " }))).toBe("Folder is required.");
    // There is no working directory the user can see, so a relative path would
    // resolve against wherever the app was launched from — a folder nobody
    // chose.
    expect(validate(folder({ share: "Documents" }))).toBe("Folder must be an absolute path.");
    expect(validate(folder({ share: "~/Documents" }))).toBe("Folder must be an absolute path.");
    // The host it does not have is not a reason to refuse it.
    expect(validate(folder({ host: "" }))).toBeNull();
  });

  it("puts the path in `share`, and leaves host, port and username empty", () => {
    // The contract with the engine: `Protocol::Local` reads its root out of
    // `share`, so a spec that put the path anywhere else would open nothing.
    const spec = toSpec(folder(), "local::/users/dorian/documents");
    expect(spec.protocol).toBe("local");
    expect(spec.share).toBe("/Users/dorian/Documents");
    expect(spec.host).toBe("");
    expect(spec.port).toBeNull();
    expect(spec.username).toBeNull();
    expect(spec.name).toBe("Documents");
  });

  it("names the whole disk something rather than nothing", () => {
    // `/` has no last segment and no host to fall back on, and a sidebar row
    // called "" is a row nobody can click on purpose.
    expect(connectionName(folder({ share: "/" }))).toBe("/");
  });

  it("keeps what still applies when the protocol tab changes", () => {
    const sftp = draft({ protocol: "sftp", host: "vault.example", username: "dorian" });
    // A server and an account mean the same thing on every network protocol.
    const nfs = switchProtocol(sftp, "nfs");
    expect(nfs.host).toBe("vault.example");
    expect(nfs.username).toBe("dorian");
    // Neither exists on Local. Carrying a hostname into a form with no Server
    // field would put it in the spec where nobody could see it.
    const local = switchProtocol(sftp, "local");
    expect(local.host).toBe("");
    expect(local.username).toBe("");
    expect(local.protocol).toBe("local");
    // And a folder is not a share name, so it does not survive either way.
    expect(switchProtocol(folder(), "smb").share).toBe("");
  });

  it("has a backend behind it in every build", () => {
    // Unlike SMB, `local` is the standard library: there is no build of this
    // app that cannot open a folder on the machine it is running on.
    expect(isImplemented("local")).toBe(true);
  });
});
