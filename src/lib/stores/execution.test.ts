import { describe, expect, test, vi } from "vite-plus/test";
import type { ExecutionAction, ExecutionSummary } from "#lib/types/index.js";
import { type ExecutionApi, ExecutionStore, isFinished } from "./execution.js";

function summary(overrides: Partial<ExecutionSummary> = {}): ExecutionSummary {
  return {
    execution_id: "exec-1",
    procedure_id: "proc",
    procedure_title: "Procedure",
    procedure_version: "1.0",
    status: "active",
    steps: [],
    execution_dir: "/tmp/exec-1",
    inbox: [],
    seq: 1,
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function fakeApi(overrides: Partial<ExecutionApi> = {}): ExecutionApi {
  return {
    startExecution: async () => summary(),
    startSharedExecution: async () => summary(),
    getExecutionState: async () => summary(),
    recordAction: async () => summary(),
    watchExecution: () => () => {},
    ...overrides,
  };
}

const rename = (name: string): ExecutionAction => ({ action: "rename_execution", name });

describe("ExecutionStore", () => {
  test("load stores the summary and clears loading", async () => {
    const store = new ExecutionStore(fakeApi(), () => ({}));

    const loaded = await store.load("exec-1");

    expect(loaded?.execution_id).toBe("exec-1");
    expect(store.getState()).toMatchObject({ loading: false, error: null });
    expect(store.getState().summary?.execution_id).toBe("exec-1");
  });

  test("load failure records the error and no summary", async () => {
    const store = new ExecutionStore(
      fakeApi({
        getExecutionState: async () => {
          throw "Execution not found";
        },
      }),
    );

    expect(await store.load("missing")).toBeNull();
    expect(store.getState()).toEqual({
      summary: null,
      loading: false,
      error: "Execution not found",
    });
  });

  test("act without a loaded execution does nothing", async () => {
    const store = new ExecutionStore(fakeApi(), () => ({}));

    expect(await store.act(rename("x"))).toBe(false);
  });

  test("actions reach the backend in order even when earlier ones are slow", async () => {
    const first = deferred<ExecutionSummary>();
    const calls: string[] = [];
    const store = new ExecutionStore(
      fakeApi({
        recordAction: async (_id, action) => {
          const name = action.action === "rename_execution" ? action.name : "";
          calls.push(name);
          return name === "first" ? first.promise : summary({ name });
        },
      }),
    );
    store.setSummary(summary());

    const firstDone = store.act(rename("first"));
    const secondDone = store.act(rename("second"));
    await Promise.resolve();
    expect(calls).toEqual(["first"]);

    first.resolve(summary({ name: "first" }));
    expect(await firstDone).toBe(true);
    expect(await secondDone).toBe(true);
    expect(calls).toEqual(["first", "second"]);
    expect(store.getState().summary?.name).toBe("second");
  });

  test("a failed action keeps the summary and later actions still run", async () => {
    const store = new ExecutionStore(
      fakeApi({
        recordAction: async (_id, action) => {
          if (action.action === "rename_execution" && action.name === "bad") throw "rejected";
          return summary({ name: "good" });
        },
      }),
    );
    store.setSummary(summary({ name: "before" }));

    expect(await store.act(rename("bad"))).toBe(false);
    expect(store.getState().error).toBe("rejected");
    expect(store.getState().summary?.name).toBe("before");

    expect(await store.act(rename("good"))).toBe(true);
    expect(store.getState()).toMatchObject({ error: null });
    expect(store.getState().summary?.name).toBe("good");
  });

  test("each action is sent with fresh metadata", async () => {
    const sent: unknown[] = [];
    let n = 0;
    const store = new ExecutionStore(
      fakeApi({
        recordAction: async (_id, _action, meta) => {
          sent.push(meta);
          return summary();
        },
      }),
      () => ({ event_id: `id-${++n}`, recorded_by: "Operator A", device: "browser-1" }),
    );
    store.setSummary(summary());

    await store.act(rename("a"));
    await store.act(rename("b"));

    expect(sent).toEqual([
      { event_id: "id-1", recorded_by: "Operator A", device: "browser-1" },
      { event_id: "id-2", recorded_by: "Operator A", device: "browser-1" },
    ]);
  });

  test("a live update ahead of the held state triggers one refetch", async () => {
    let onSeq: (seq: number) => void = () => {};
    let fetches = 0;
    const store = new ExecutionStore(
      fakeApi({
        watchExecution: (_id, callback) => {
          onSeq = callback;
          return () => {};
        },
        getExecutionState: async () => {
          fetches++;
          return summary({ seq: 5, name: "from another client" });
        },
      }),
      () => ({}),
    );
    store.setSummary(summary({ seq: 4 }));
    store.watch("exec-1");

    onSeq(4); // not ahead: ignored
    onSeq(5);
    await vi.waitFor(() => expect(store.getState().summary?.seq).toBe(5));

    expect(fetches).toBe(1);
    expect(store.getState().summary?.name).toBe("from another client");
  });

  test("live updates for another execution are ignored", () => {
    let onSeq: (seq: number) => void = () => {};
    let fetches = 0;
    const store = new ExecutionStore(
      fakeApi({
        watchExecution: (_id, callback) => {
          onSeq = callback;
          return () => {};
        },
        getExecutionState: async () => {
          fetches++;
          return summary();
        },
      }),
      () => ({}),
    );
    store.setSummary(summary({ execution_id: "exec-2", seq: 1 }));
    store.watch("exec-1");

    onSeq(9);

    expect(fetches).toBe(0);
  });

  test("an older response never replaces a newer state", async () => {
    const slowAction = deferred<ExecutionSummary>();
    const store = new ExecutionStore(
      fakeApi({
        recordAction: () => slowAction.promise,
        getExecutionState: async () => summary({ seq: 7, name: "newer" }),
      }),
      () => ({}),
    );
    store.setSummary(summary({ seq: 5 }));

    const acted = store.act(rename("mine"));
    await store.refresh();
    slowAction.resolve(summary({ seq: 6, name: "mine" }));
    await acted;

    expect(store.getState().summary).toMatchObject({ seq: 7, name: "newer" });
  });

  test("overlapping refreshes are coalesced", async () => {
    const first = deferred<ExecutionSummary>();
    let fetches = 0;
    const store = new ExecutionStore(
      fakeApi({
        getExecutionState: () => {
          fetches++;
          return fetches === 1 ? first.promise : Promise.resolve(summary({ seq: 3 }));
        },
      }),
      () => ({}),
    );
    store.setSummary(summary({ seq: 1 }));

    const running = store.refresh();
    void store.refresh();
    void store.refresh();
    first.resolve(summary({ seq: 2 }));
    await running;

    expect(fetches).toBe(2);
    expect(store.getState().summary?.seq).toBe(3);
  });

  test("subscribers are notified with a new snapshot on every change", () => {
    const store = new ExecutionStore(fakeApi(), () => ({}));
    const snapshots: unknown[] = [];
    const unsubscribe = store.subscribe(() => snapshots.push(store.getState()));
    const before = store.getState();

    store.setError("oops");
    unsubscribe();
    store.reset();

    expect(snapshots).toHaveLength(1);
    expect(snapshots[0]).not.toBe(before);
  });
});

test.each([
  ["active", false],
  ["pass", true],
  ["fail", true],
  ["aborted", true],
])("isFinished(%s) is %s", (status, expected) => {
  expect(isFinished(summary({ status }))).toBe(expected);
});
