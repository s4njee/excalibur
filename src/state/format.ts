// Presentation helpers. Kept apart from the components so the exact strings
// the design specifies are testable without rendering anything.

/**
 * Bytes as the Size column shows them, in decimal units.
 *
 * One decimal place, dropped when it would be `.0`: the design shows both
 * "2.4 TB" and "16 TB" in the same sentence, so a fixed number of decimals
 * matches neither. Three digits displace the decimal entirely ("999 GB").
 */
export function formatSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // Only an exact whole number loses its decimal: "16 TB" is exactly 16, while
  // 44.98 GB is not 45 and should not claim to be.
  const whole = Number.isInteger(value);
  const rendered = value >= 100 || whole ? value.toFixed(0) : value.toFixed(1);
  return `${rendered} ${units[unit]}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** The Modified column's format: `31 Aug 2026, 18:24`. */
export function formatModified(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  const day = date.getDate();
  const month = MONTHS[date.getMonth()];
  const hours = `${date.getHours()}`.padStart(2, "0");
  const minutes = `${date.getMinutes()}`.padStart(2, "0");
  return `${day} ${month} ${date.getFullYear()}, ${hours}:${minutes}`;
}

/** The short form the dense row layout uses: `31 Aug 18:24`. */
export function formatModifiedShort(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  const hours = `${date.getHours()}`.padStart(2, "0");
  const minutes = `${date.getMinutes()}`.padStart(2, "0");
  return `${date.getDate()} ${MONTHS[date.getMonth()]} ${hours}:${minutes}`;
}

/**
 * The status bar's counts: `13 items · 1 selected · 48.2 GB`.
 *
 * The size is of the *selection*, not the directory — the design is explicit
 * about that, and showing the total instead would be a plausible-looking lie.
 *
 * When a filter is hiding rows the count says `3 of 13 items`. The AC asks for
 * counts that reflect the filtered set, and a bare `3 items` does reflect it —
 * but it also reads exactly like a directory that contains three files, which
 * is the one thing the user must not conclude while a filter is on.
 */
export function formatCounts(
  items: number,
  selected: number,
  selectedBytes: number,
  total?: number,
): string {
  const filtered = total !== undefined && total !== items;
  const parts = [
    filtered
      ? `${items} of ${total} item${total === 1 ? "" : "s"}`
      : `${items} item${items === 1 ? "" : "s"}`,
  ];
  if (selected > 0) {
    parts.push(`${selected} selected`, formatSize(selectedBytes));
  }
  return parts.join(" · ");
}

/** The capacity phrase: `2.4 TB free of 16 TB`. */
export function formatCapacity(free: number, total: number): string {
  return `${formatSize(free)} free of ${formatSize(total)}`;
}
