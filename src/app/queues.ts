// The window's view of the app-wide transfer queue (E13, XDL-E2).
//
// The queue itself lives in Rust; this holds what the window has been told
// about it and the calls that change it. Split out of `App` because it is a
// closed set: every function here is `backend` call → refresh → report, and
// none of them touches the file list, the selection or the mount.
//
// Progress arrives as a broadcast — every window hears about every transfer,
// because a transfer belongs to the app rather than to the window that started
// it. `onSettled` is how the window decides whether one is any of its business.

import { createSignal, onCleanup, onMount } from "solid-js";
import { engineFailure } from "../state/errors";
import type { Backend } from "../state/backend";
import type { NamedQueue, Transfer } from "../state/types";

export interface QueueDeps {
  backend: Backend;
  /** Connection-level failures go to the window's banner. */
  onProblem: (message: string) => void;
  /** A transfer reached a terminal state, whatever window started it. */
  onSettled: (transfer: Transfer) => void;
}

export function createQueues(deps: QueueDeps) {
  const [transfers, setTransfers] = createSignal<Transfer[]>([]);
  const [queues, setQueues] = createSignal<NamedQueue[]>([]);

  const refreshTransfers = () => void deps.backend.transfers().then(setTransfers);
  const refreshQueues = () => void deps.backend.downloadQueues().then(setQueues);

  /** Every queue verb is this shape, so the error path is written once. */
  const attempt = async (run: () => Promise<void>) => {
    try {
      await run();
    } catch (error) {
      deps.onProblem(engineFailure(error).message);
    }
  };

  const createQueue = (name: string, color: string, concurrency: number) =>
    attempt(async () => {
      await deps.backend.createDownloadQueue(name, color, concurrency);
      refreshQueues();
    });

  const updateQueue = (id: string, name: string, color: string, concurrency: number) =>
    attempt(async () => {
      await deps.backend.updateDownloadQueue(id, name, color, concurrency);
      refreshQueues();
    });

  const deleteQueue = (id: string) =>
    attempt(async () => {
      await deps.backend.deleteDownloadQueue(id);
      refreshQueues();
      // The rows that were in it are now somewhere else.
      refreshTransfers();
    });

  const moveTransferToQueue = (id: string, queueId: string) =>
    attempt(async () => {
      const changed = await deps.backend.moveTransferToQueue(id, queueId);
      // The one verb that can answer with the whole row, so it does not need a
      // round trip to show its own result.
      setTransfers((current) =>
        current.map((transfer) => (transfer.id === changed.id ? changed : transfer)),
      );
    });

  const reorderQueue = (queueId: string, ids: string[]) =>
    attempt(async () => {
      await deps.backend.reorderDownloadQueue(queueId, ids);
      refreshTransfers();
    });

  const reorderQueues = (ids: string[]) =>
    attempt(async () => {
      await deps.backend.reorderDownloadQueues(ids);
      refreshQueues();
      refreshTransfers();
    });

  onMount(() => {
    refreshTransfers();
    refreshQueues();
    const stop = deps.backend.onTransferChanged((changed) => {
      setTransfers((current) => {
        const index = current.findIndex((transfer) => transfer.id === changed.id);
        if (index === -1) return [...current, changed];
        const next = current.slice();
        next[index] = changed;
        return next;
      });
      deps.onSettled(changed);
    });
    onCleanup(stop);
    const stopQueues = deps.backend.onQueueChanged(setQueues);
    onCleanup(stopQueues);
  });

  return {
    transfers,
    setTransfers,
    queues,
    refreshTransfers,
    refreshQueues,
    createQueue,
    updateQueue,
    deleteQueue,
    moveTransferToQueue,
    reorderQueue,
    reorderQueues,
  };
}
