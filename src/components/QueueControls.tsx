import { For, Show, createMemo, createSignal } from "solid-js";
import styles from "./transfers.module.css";
import type { NamedQueue } from "../state/types";

/** Queue picker and the small CRUD editor used by the Downloads surface. */
export function QueueControls(props: {
  queues: NamedQueue[];
  selected: string;
  onSelect: (id: string) => void;
  onCreate: (name: string, color: string, concurrency: number) => void;
  onUpdate: (id: string, name: string, color: string, concurrency: number) => void;
  onDelete: (id: string) => void;
  onReorder: (ids: string[]) => void;
}) {
  const [open, setOpen] = createSignal(false);
  const [editing, setEditing] = createSignal<string | null>(null);
  const [name, setName] = createSignal("");
  const [color, setColor] = createSignal("blue");
  const [concurrency, setConcurrency] = createSignal("1");
  const ordered = createMemo(() => props.queues.slice().sort((a, b) => a.order - b.order));

  const beginCreate = () => {
    setEditing(null);
    setName("");
    setColor("blue");
    setConcurrency("1");
    setOpen(true);
  };
  const beginEdit = (queue: NamedQueue) => {
    setEditing(queue.id);
    setName(queue.name);
    setColor(queue.color);
    setConcurrency(String(queue.concurrency));
    setOpen(true);
  };
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    const trimmed = name().trim();
    const limit = Math.max(1, Number.parseInt(concurrency(), 10) || 1);
    if (!trimmed) return;
    const id = editing();
    if (id) props.onUpdate(id, trimmed, color().trim() || "blue", limit);
    else props.onCreate(trimmed, color().trim() || "blue", limit);
    setOpen(false);
  };
  const moveQueue = (id: string, direction: -1 | 1) => {
    const ids = ordered().map((queue) => queue.id);
    const index = ids.indexOf(id);
    const next = index + direction;
    if (index < 0 || next < 0 || next >= ids.length) return;
    [ids[index], ids[next]] = [ids[next]!, ids[index]!];
    props.onReorder(ids);
  };

  return (
    <div class={styles.queueControls} role="group" aria-label="Download queues">
      <label class={styles.queuePicker}>
        <span>Queue</span>
        <select
          aria-label="Download queue"
          value={props.selected}
          onChange={(event) => props.onSelect(event.currentTarget.value)}
        >
          <option value="all">All queues</option>
          <For each={ordered()}>{(queue) => <option value={queue.id}>{queue.name}</option>}</For>
        </select>
      </label>
      <button class={styles.headerAction} onClick={() => setOpen((value) => !value)}>
        {open() ? "Done" : "Manage queues"}
      </button>
      <Show when={open()}>
        <div class={styles.queueEditor} role="dialog" aria-label="Manage download queues">
          <div class={styles.queueEditorTitle}>Download queues</div>
          <For each={ordered()}>
            {(queue, index) => (
              <div class={styles.queueDefinition} data-queue={queue.id}>
                <span class={styles.queueSwatch} style={{ "background-color": `var(--queue-${queue.color}, var(--accent))` }} />
                <span class={styles.queueDefinitionName}>{queue.name}</span>
                <span class={styles.queueDefinitionMeta}>{queue.concurrency} at a time</span>
                <button class={styles.queueButton} onClick={() => beginEdit(queue)}>Edit</button>
                <button
                  class={styles.queueButton}
                  aria-label={`Move ${queue.name} up`}
                  disabled={index() === 0}
                  onClick={() => moveQueue(queue.id, -1)}
                >↑</button>
                <button
                  class={styles.queueButton}
                  aria-label={`Move ${queue.name} down`}
                  disabled={index() === ordered().length - 1}
                  onClick={() => moveQueue(queue.id, 1)}
                >↓</button>
                <button
                  class={styles.queueButton}
                  disabled={queue.id === "default"}
                  title={queue.id === "default" ? "Default cannot be deleted" : undefined}
                  onClick={() => {
                    if (window.confirm(`Delete ${queue.name}? Its downloads will move to Default.`)) {
                      props.onDelete(queue.id);
                    }
                  }}
                >Delete</button>
              </div>
            )}
          </For>
          <Show when={open()}>
            <form class={styles.queueForm} onSubmit={submit}>
              <div class={styles.queueFormTitle}>{editing() ? "Edit queue" : "New queue"}</div>
              <input aria-label="Queue name" placeholder="Queue name" value={name()} onInput={(event) => setName(event.currentTarget.value)} />
              <input aria-label="Queue colour" placeholder="Colour" value={color()} onInput={(event) => setColor(event.currentTarget.value)} />
              <input aria-label="Queue concurrency" type="number" min="1" max="256" value={concurrency()} onInput={(event) => setConcurrency(event.currentTarget.value)} />
              <button type="submit" class={styles.queueButton}>{editing() ? "Save queue" : "Create queue"}</button>
              <Show when={editing()}><button type="button" class={styles.queueButton} onClick={beginCreate}>New instead</button></Show>
            </form>
          </Show>
          <button class={styles.queueAdd} onClick={beginCreate}>+ New queue</button>
        </div>
      </Show>
    </div>
  );
}
