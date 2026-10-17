import { useEffect, useState, useSyncExternalStore } from "react";
import { getMe } from "#lib/api/commands.js";
import { operatorName, setOperatorName, subscribeOperatorName } from "#lib/operator.js";
import { scoped } from "#lib/utils/cx.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import styles from "./OperatorName.module.css";

const c = scoped(styles);

/**
 * Who is recording. Behind Cloudflare Access the server records the
 * signed-in email, so it is shown as is. Otherwise the operator enters a
 * name, recorded with every action and editable in place.
 */
export default function OperatorName() {
  const name = useSyncExternalStore(subscribeOperatorName, operatorName);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(name);
  const [signedInAs, setSignedInAs] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getMe().then((me) => {
      if (!cancelled) setSignedInAs(me.email);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (signedInAs) {
    return (
      <span className={c("operator")} title="Recorded with your actions">
        <span className={c("label")}>Signed in as</span>
        <span className={c("name")}>{signedInAs}</span>
      </span>
    );
  }

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
