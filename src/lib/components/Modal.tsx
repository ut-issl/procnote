import type { ReactNode } from "react";
import { scoped } from "#lib/utils/cx.js";
import styles from "./Modal.module.css";

const c = scoped(styles);

export default function Modal({
  children,
  oncancel,
}: {
  children: ReactNode;
  oncancel: () => void;
}) {
  return (
    <div className={c("modal-backdrop")} role="presentation" onClick={oncancel}>
      <div className={c("modal")} onClick={(e) => e.stopPropagation()}>
        {children}
      </div>
    </div>
  );
}
