import { describe, expect, it } from "vitest";
import { engineFailure } from "./errors";

describe("engineFailure", () => {
  it("reads a serialised CommandError rather than stringifying it", () => {
    const failure = engineFailure({
      message: "connection lost: reset by peer",
      surface: "banner",
      transient: true,
    });
    expect(failure.message).toBe("connection lost: reset by peer");
    expect(failure.surface).toBe("banner");
    expect(failure.transient).toBe(true);
  });

  it("does not turn a plain object into [object Object]", () => {
    expect(engineFailure({ message: "nope", surface: "field", transient: false }).message).not.toContain(
      "[object Object]",
    );
  });

  it("keeps a thrown Error readable and gives it a harmless surface", () => {
    const failure = engineFailure(new Error("no engine here"));
    expect(failure.message).toContain("no engine here");
    // Not a banner and not a prompt: an unclassified failure should not raise
    // either of the two surfaces that interrupt the user.
    expect(failure.surface).toBe("transfer-row");
  });

  it("carries an unknown host key as a prompt with no expected fingerprint", () => {
    const failure = engineFailure({
      message: "unknown host key for nas.local: ssh-ed25519 SHA256:abc",
      surface: "trust-prompt",
      transient: false,
      hostKey: { host: "nas.local", port: 22, algorithm: "ssh-ed25519", fingerprint: "SHA256:abc" },
    });
    expect(failure.surface).toBe("trust-prompt");
    expect(failure.hostKey?.fingerprint).toBe("SHA256:abc");
    expect(failure.hostKey?.expected).toBeUndefined();
  });

  it("carries a changed host key as a banner that names both fingerprints", () => {
    const failure = engineFailure({
      message: "host key for nas.local has changed",
      surface: "banner",
      transient: false,
      hostKey: {
        host: "nas.local",
        port: 22,
        algorithm: "ssh-ed25519",
        fingerprint: "SHA256:new",
        expected: "SHA256:old",
      },
    });
    expect(failure.surface).toBe("banner");
    expect(failure.hostKey?.expected).toBe("SHA256:old");
    expect(failure.hostKey?.fingerprint).toBe("SHA256:new");
  });
});
