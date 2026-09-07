import styles from "./sparkline.module.css";
import { sparklinePath } from "../state/throughput";

/**
 * The downloads footer's sparkline (XDL-E2-S4): the last 60 samples of
 * aggregate throughput as a tiny area line. Purely decorative — the numbers
 * sit beside it, and it is not the only place a figure appears.
 */
export function Sparkline(props: { samples: number[]; width?: number; height?: number }) {
  const width = props.width ?? 120;
  const height = props.height ?? 24;
  const path = sparklinePath(props.samples, width, height);
  return (
    <svg
      class={styles.sparkline}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-hidden="true"
    >
      <path d={path} class={styles.line} />
    </svg>
  );
}
