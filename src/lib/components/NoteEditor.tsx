import { useState } from "react";
import type { NoteState } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import TrashIcon from "./TrashIcon.js";
import styles from "./NoteEditor.module.css";

const c = scoped(styles);

export default function NoteEditor({
  notes,
  disabled = false,
  onadd,
  onrevert,
}: {
  notes: NoteState[];
  disabled?: boolean;
  onadd: (text: string) => void;
  onrevert?: (noteId: string) => void;
}) {
  const [noteText, setNoteText] = useState("");

  function submit() {
    if (!noteText.trim()) return;
    onadd(noteText.trim());
    setNoteText("");
  }

  return (
    <div className={c("note-editor")}>
      {notes.length > 0 && (
        <ul className={c("note-list")}>
          {notes.map((note) => (
            <li className={c("note-item")} key={note.id}>
              <span className={c("note-text")}>{note.text}</span>
              {note.at && <span className="timestamp">{formatTimestamp(note.at)}</span>}
              {onrevert && (
                <button
                  className={c("btn-delete")}
                  title="Delete note"
                  onClick={() => onrevert(note.id)}
                >
                  <TrashIcon />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!disabled && (
        <div className={c("note-input")}>
          <input
            type="text"
            value={noteText}
            onChange={(e) => setNoteText(e.target.value)}
            placeholder="Add a note..."
            onKeyDown={(e) => {
              if (isNonComposingEnter(e.nativeEvent)) submit();
            }}
          />
          <button className={c("btn-add")} onClick={submit} disabled={!noteText.trim()}>
            Add
          </button>
        </div>
      )}
    </div>
  );
}
