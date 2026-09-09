// The Add Download dialog and what it starts (XDL-E3-S1, XDL-E8-S1).
//
// Split out of `App` because it is self-contained: four signals nobody else
// reads, one resolver for the default destination, and one submit path. The
// window only needs to know how to open it and what it is showing.

import { createSignal } from "solid-js";
import { engineFailure } from "../state/errors";
import type { Backend } from "../state/backend";
import type { Prefs } from "../state/prefs";
import type { Settings } from "../state/settings";
import type { HttpDownloadOptions } from "../state/types";

export interface AddDownloadDeps {
  backend: Backend;
  prefs: Prefs;
  /** This window's label, so the last-used destination is remembered per window. */
  label: string;
  settings: () => Settings;
  /** Re-read the queue once the batch is away. */
  refreshTransfers: () => void;
  onProblem: (message: string) => void;
  onNotice: (message: string) => void;
}

export function createAddDownload(deps: AddDownloadDeps) {
/**
 * The Add Download dialog (XDL-E3-S1). `addingDest` is the resolved default
 * folder: the configured download folder, else this window's last-used
 * destination, else the OS folder the engine names — each may need an async
 * answer, so it is resolved before the dialog opens rather than by it.
 */
const [addingDownload, setAddingDownload] = createSignal(false);
const [addingDest, setAddingDest] = createSignal("");
const [addingUrls, setAddingUrls] = createSignal<string[]>([]);
const [addingBusy, setAddingBusy] = createSignal(false);
const downloadDestKey = () => `download-dest:${deps.label}`;
const openAddDownload = async (initialUrls: string[] = []) => {
  setAddingUrls(initialUrls);
  try {
    const stored = await deps.prefs.get(downloadDestKey());
    if (typeof stored === "string" && stored.trim() !== "") {
      setAddingDest(stored);
      setAddingDownload(true);
      return;
    }
    const downloadFolder = deps.settings().downloadFolder;
    if (downloadFolder !== null) {
      setAddingDest(downloadFolder);
      setAddingDownload(true);
      return;
    }
    setAddingDest((await deps.backend.downloadFolder()) ?? "");
  } catch {
    setAddingDest("");
  }
  setAddingDownload(true);
};
const addDownloads = async (
  urls: string[],
  folder: string,
  options: HttpDownloadOptions[] = [],
) => {
  // Remembered before the rows are: the folder is the part the queue would
  // own, and queues do not exist yet, so it is remembered per window label
  // instead (XDL-E6-S1 is the real home).
  void deps.prefs.set(downloadDestKey(), folder);
  setAddingBusy(true);
  let started = 0;
  let failure: string | null = null;
  // Mirror mode represents one object: the first URL is primary and the
  // remaining URLs are failover sources carried in its options.
  const jobs = options[0]?.mirrors?.length
    ? [{ url: urls[0], options: options[0] }]
    : urls.map((url, index) => ({ url, options: options[index] }));
  for (const item of jobs) {
    if (!item.url) continue;
    try {
      // The Preferences value is the per-download default. An explicit
      // choice in Add Download wins, while `null` preserves the provider's
      // automatic selection.
      const configured = deps.settings().downloadConnections;
      const withDefault: HttpDownloadOptions = {
        ...item.options,
        ...(configured !== null && item.options?.connections === undefined
          ? { connections: configured }
          : {}),
        retry: deps.settings().downloadRetry,
      };
      const override =
        Object.keys(withDefault).length > 0 ? withDefault : undefined;
      await deps.backend.enqueueHttpDownload(item.url, folder, override);
      started += 1;
    } catch (error) {
      failure ??= engineFailure(error).message;
    }
  }
  setAddingBusy(false);
  setAddingDownload(false);
  deps.refreshTransfers();
  if (failure !== null) {
    deps.onProblem(`Started ${started} of ${jobs.length} downloads — ${failure}`);
  } else {
    deps.onNotice(started === 1 ? "Download started" : `${started} downloads started`);
  }
};
  return {
    open: openAddDownload,
    submit: addDownloads,
    showing: addingDownload,
    close: () => {
      if (!addingBusy()) setAddingDownload(false);
    },
    destination: addingDest,
    urls: addingUrls,
    busy: addingBusy,
  };
}
