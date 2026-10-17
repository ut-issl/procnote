import { useState, useSyncExternalStore } from "react";
import { operatorName, setOperatorName, subscribeOperatorName } from "#lib/operator.js";
import { scoped } from "#lib/utils/cx.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import styles from "./OperatorName.module.css";

const c = scoped(styles);

/** The operator's name, recorded with every action; editable in place. */
export default function OperatorName() {
  const name = useSyncExternalStore(subscribeOperatorName, operatorName);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(name);

  function commit() {
    setOperatorName(draft);
    setEditing(false);
  }

  if (editing || !name) {
    return (
      <label className={c("operator", !name && "missing")}>
        <span className={c("label")}>Your name</span>
        <input
          value={draft}
          placeholder="Recorded with your actions"
          autoFocus={editing}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (isNonComposingEnter(e.nativeEvent)) commit();
            if (e.key === "Escape") {
              setDraft(name);
              setEditing(false);
            }
          }}
        />
      </label>
    );
  }
  return (
    <button
      className={c("operator", "set")}
      title="Change your name"
      onClick={() => {
        setDraft(name);
        setEditing(true);
      }}
    >
      <span className={c("label")}>Recording as</span>
      <span className={c("name")}>{name}</span>
    </button>
  );
}
