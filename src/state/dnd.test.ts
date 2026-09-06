import { describe, expect, it } from "vitest";
import {
  describeDrop,
  moveTargets,
  osPath,
  resolveDrop,
  SPRING_DELAY_MS,
  type DropMount,
  type DropPlan,
  type DropRequest,
} from "./dnd";

const raid6: DropMount = { id: "xsync:raid6:media", protocol: "xsync", root: "" };
const nas: DropMount = { id: "smb:nas.local:media", protocol: "smb", root: "" };
const home: DropMount = { id: "local::/users/dorian", protocol: "local", root: "/Users/dorian" };
const scratch: DropMount = { id: "local::/tmp/scratch", protocol: "local", root: "/tmp/scratch" };

/** A drag of `paths` from `from`, dropped into `path` on `to`. */
function drop(options: Partial<DropRequest> & { paths: string[] }): DropPlan {
  const from = options.from === undefined ? raid6 : options.from;
  return resolveDrop({
    selection: {
      mount: options.selection?.mount ?? from?.id ?? "",
      paths: options.paths,
      window: options.selection?.window ?? "main",
    },
    from,
    to: options.to ?? raid6,
    path: options.path ?? "Documentaries",
    writable: options.writable ?? true,
    alt: options.alt ?? false,
  });
}

describe("resolveDrop, on one mount", () => {
  it("moves, because the same share is a rename and not a copy", () => {
    const plan = drop({ paths: ["Films/Arrival.mkv"] });
    expect(plan).toEqual({
      verb: "move",
      mount: raid6.id,
      destination: "Documentaries",
      items: [{ path: "Films/Arrival.mkv", name: "Arrival.mkv" }],
    });
  });

  it("does nothing, silently, when it lands back where it started", () => {
    // Not a refusal: the user let go over the folder they were already in,
    // which is how a drag is cancelled. A red ring for that would read as an
    // error the user did not make.
    expect(drop({ paths: ["Films/Arrival.mkv"], path: "Films" })).toEqual({ verb: "none" });
  });

  it("refuses a folder into itself, in E14-S3's words", () => {
    // The failure that actually corrupts a share, and the reason `move.ts` is
    // reused here rather than re-derived: two implementations of "is this
    // inside that" is one implementation too many.
    expect(drop({ paths: ["Films"], path: "Films" })).toEqual({
      verb: "refused",
      reason: "You cannot move “Films” into itself.",
    });
    expect(drop({ paths: ["Films"], path: "Films/2024" })).toEqual({
      verb: "refused",
      reason: "You cannot move “Films” into a folder inside it.",
    });
  });

  it("does not mistake a neighbouring folder for a child", () => {
    // `Films-old` is not inside `Films`, however much the prefixes agree.
    expect(drop({ paths: ["Films"], path: "Films-old" }).verb).toBe("move");
  });

  it("moves into the share root, which is a folder like any other", () => {
    const plan = drop({ paths: ["Films/Arrival.mkv"], path: "" });
    expect(plan).toEqual({
      verb: "move",
      mount: raid6.id,
      destination: "",
      items: [{ path: "Films/Arrival.mkv", name: "Arrival.mkv" }],
    });
  });
});

describe("resolveDrop, with ⌥ held", () => {
  it("copies within one local share, through the download verb", () => {
    // Local on both sides, so the queue can actually do it: read the share,
    // write the disk. Same folder is no longer a no-op — ⌥ makes it a
    // deliberate duplicate.
    const plan = drop({
      paths: ["Documents/notes.md"],
      from: home,
      to: home,
      path: "Archive",
      alt: true,
    });
    expect(plan).toEqual({
      verb: "copy",
      direction: "download",
      sourceMount: home.id,
      destMount: home.id,
      destination: "/Users/dorian/Archive",
      sources: ["Documents/notes.md"],
      items: [{ path: "Documents/notes.md", name: "notes.md" }],
    });
  });

  it("refuses a ⌥ copy inside a remote share, and says why", () => {
    // Downloading and re-uploading a file to the share it is already on is not
    // a copy anyone asked for, and there is no server-side copy verb until
    // E22. Refusing beats queueing something that would half-work.
    expect(drop({ paths: ["Films/Arrival.mkv"], alt: true })).toEqual({
      verb: "refused",
      reason: "Copying between two shares arrives with E22.",
    });
  });
});

describe("resolveDrop, across two mounts", () => {
  it("copies onto a local share with the download verb", () => {
    const plan = drop({ paths: ["Films/Arrival.mkv"], from: raid6, to: home, path: "Movies" });
    expect(plan).toMatchObject({
      verb: "copy",
      direction: "download",
      sourceMount: raid6.id,
      destMount: home.id,
      destination: "/Users/dorian/Movies",
      sources: ["Films/Arrival.mkv"],
    });
  });

  it("copies off a local share with the upload verb, in OS paths", () => {
    // The queue's upload takes *absolute local* sources, so the share-relative
    // paths on the pasteboard have to be rooted at the local mount before they
    // mean anything. Handing it `Downloads/x.iso` would upload from the
    // process's working directory.
    const plan = drop({
      paths: ["Downloads/ubuntu.iso"],
      from: home,
      to: raid6,
      path: "Installers",
    });
    expect(plan).toMatchObject({
      verb: "copy",
      direction: "upload",
      sources: ["/Users/dorian/Downloads/ubuntu.iso"],
      destination: "Installers",
    });
  });

  it("prefers the download verb when both sides are local", () => {
    // Two local shares can go either way; download is chosen because it is the
    // only one of the two that walks a folder.
    expect(drop({ paths: ["Documents"], from: home, to: scratch, path: "" })).toMatchObject({
      verb: "copy",
      direction: "download",
      destination: "/tmp/scratch",
    });
  });

  it("refuses share to share, because the queue cannot hold two backends", () => {
    expect(drop({ paths: ["Films/Arrival.mkv"], from: raid6, to: nas, path: "in" })).toEqual({
      verb: "refused",
      reason: "Copying between two shares arrives with E22.",
    });
  });

  it("refuses when the share the drag came from is no longer held", () => {
    // The source window closed its share mid-drag. There is nothing to read
    // the bytes from, and saying so beats a queue row that fails later.
    expect(
      drop({ paths: ["Films/Arrival.mkv"], from: null, to: home, path: "" }),
    ).toEqual({ verb: "refused", reason: "That share is no longer open." });
  });

  it("refuses a read-only destination rather than highlighting it", () => {
    expect(
      drop({ paths: ["Films/Arrival.mkv"], from: raid6, to: nas, path: "", writable: false }),
    ).toEqual({ verb: "refused", reason: "That share is mounted read-only." });
  });
});

describe("describeDrop", () => {
  it("says the verb and the place, with a count only when there is one", () => {
    const one = drop({ paths: ["Films/Arrival.mkv"] });
    expect(describeDrop(one, "Documentaries")).toBe("Move to Documentaries");

    const three = drop({
      paths: ["a.mkv", "b.mkv", "c.mkv"],
      from: home,
      to: raid6,
      path: "in",
    });
    expect(describeDrop(three, "raid6")).toBe("Copy 3 items to raid6");
    const single = drop({ paths: ["a.mkv"], from: home, to: raid6, path: "in" });
    expect(describeDrop(single, "raid6")).toBe("Copy to raid6");
  });

  it("shows a refusal verbatim, because the reason is the whole point", () => {
    expect(describeDrop(drop({ paths: ["Films"], path: "Films" }), "Films")).toBe(
      "You cannot move “Films” into itself.",
    );
  });

  it("says nothing at all for a drop that does nothing", () => {
    expect(describeDrop({ verb: "none" }, "Films")).toBe("");
  });
});

describe("moveTargets", () => {
  it("keeps the leaf and replaces everything above it", () => {
    const plan = drop({ paths: ["Films/2024/Arrival.mkv", "Films/2024/Dune"] });
    expect(plan.verb).toBe("move");
    expect(moveTargets(plan as Extract<DropPlan, { verb: "move" }>)).toEqual([
      { from: "Films/2024/Arrival.mkv", to: "Documentaries/Arrival.mkv" },
      { from: "Films/2024/Dune", to: "Documentaries/Dune" },
    ]);
  });
});

describe("osPath", () => {
  it("roots a share-relative path, and the root itself has no trailing slash", () => {
    expect(osPath(home, "Movies/2024")).toBe("/Users/dorian/Movies/2024");
    expect(osPath(home, "")).toBe("/Users/dorian");
    expect(osPath({ ...home, root: "/Users/dorian/" }, "Movies")).toBe("/Users/dorian/Movies");
  });
});

describe("the spring-loaded delay", () => {
  it("is Finder's, so a drag that pauses over a folder behaves the same way", () => {
    expect(SPRING_DELAY_MS).toBe(600);
  });
});
