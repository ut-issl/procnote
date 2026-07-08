---
icon: material/play-circle
---

# Running an Execution

Once you have a [procedure template](writing-a-template.md), you can execute it in Procnote.

## Starting an Execution

1. Launch Procnote with a workspace directory:

    ```bash
    procnote /path/to/my-workspace
    ```

    If omitted, Procnote uses the current working directory.
2. The home screen lists all procedure templates found in the workspace. Click on a template to view its details.
3. Click **Start Execution** to begin.

Procnote creates an execution directory under `.executions/` with:

- `events.jsonl` -- The append-only event log
- `template.md` -- A snapshot of the template at the time of execution

## Working Through Steps

Each step in the procedure is displayed as a card. To work through a step:

1. **Start the step** -- Click to mark it as active.
2. **Complete checklist items** -- Toggle checkboxes as you verify each condition.
3. **Record inputs** -- Enter measurements, select options, or attach files.
4. **Add notes** -- Attach freeform notes to any step or to the execution as a whole.
5. **Complete the step** -- Mark it as done, or skip it with a reason.

## Attachment Uploads from Another Device

If [DropPoint integration](drop-point.md) is configured, attachment inputs also show **Upload via QR Code**. Use it when a sender needs to upload files from another device, such as photos from a phone. Procnote creates a short-lived DropPoint upload session, imports the encrypted upload as local attachments, and then closes the remote drop point.

## Adding Steps During Execution

You can add new steps dynamically during an execution using the **+ Add Step** button in the toolbar. The new step is inserted after the currently selected step (or at the end).

## Completing or Aborting

When all steps are done:

- Click **Complete** and choose **Pass** or **Fail** to finish the execution.
- Click **Abort** to stop early with a reason.

## Reversing Actions

Procnote supports reversing operator actions through normal UI actions:

- Clear a recorded input before recording a new value.
- Remove an attachment file or note.
- Unskip a skipped step.
- Reopen a completed or aborted execution.
- Toggle a checkbox back to its previous value.

The original event is not deleted. A typed reversing event such as `InputCleared`, `StepUnskipped`, or `ExecutionReopened` is appended to the log, preserving the full audit trail. Execution state is rebuilt by replaying every event in order.

!!! info "Not all actions are reversible"

    Structural events like `ExecutionStarted` and `StepAdded` are not reversed by normal operator actions.

## Reviewing Past Executions

The home screen shows recent executions with their status (active, pass, fail, aborted). Click on any execution to view its full event history and recorded data.

## Storage Layout

Each execution is stored in its own directory:

```text
my-workspace/
└── my-procedure/
    ├── template.md
    └── .executions/
        └── 20260402T033055-edb182e8/
            ├── events.jsonl
            ├── template.md
            └── attachments/
                ├── a1b2c3d-report.pdf
                └── d4e5f6a-photo.jpg
```

The `events.jsonl` file is the single source of truth. It is append-only and never modified after creation.
