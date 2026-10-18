import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, test } from "vite-plus/test";
import type { PushResult } from "#lib/api/commands.js";
import type { ExecutionApi } from "#lib/stores/execution.js";
import type { EventRecord, ExecutionSummary, ProcedureTemplate } from "#lib/types/index.js";
import * as core from "#lib/wasm/core.js";
import { type KeyValueStore, LocalExecutions, type SyncServer } from "./localExecutions.js";

const TEMPLATE: ProcedureTemplate = {
  metadata: { id: "LOCAL-1", title: "Local", version: "1", equipment: [], requirement_traces: [] },
  steps: [
    {
      heading: "Only step",
      content: [
        { type: "Checkbox", text: "First", checked: false },
        { type: "Checkbox", text: "Second", checked: false },
      ],
    },
  ],
};
const PATH = "/workspace/local-test/template.md";

beforeAll(() => {
  core.loadCoreFrom(readFileSync(new URL("../wasm/pkg/procnote_wasm_bg.wasm", import.meta.url)));
});

const realCore = {
  ready: () => Promise.resolve(),
  summarize: core.summarize,
  decide: core.decide,
  start: core.start,
};

function memoryStore(): KeyValueStore {
  const values = new Map<string, unknown>();
  return {
    get: async <T>(key: string) => structuredClone(values.get(key)) as T | undefined,
    set: async (key, value) => {
      values.set(key, structuredClone(value));
    },
    entries: async <T>() =>
      [...values.entries()].map(([k, v]) => [k, structuredClone(v)] as [string, T]),
  };
}

/**
 * A server that behaves like procnote-server's sync API: it deduplicates by
 * event ID and keeps events that no longer apply as conflicts. It can be cut
 * off, failing like an unreachable network.
 */
class FakeServer implements SyncServer {
  logs = new Map<string, EventRecord[]>();
  conflicts: EventRecord[] = [];
  reachable = true;

  #check() {
    if (!this.reachable) throw new TypeError("Failed to fetch");
  }

  async pullEvents(id: string, after: number) {
    this.#check();
    const log = this.logs.get(id);
    if (!log) throw `Execution not found: ${id}`;
    return { records: structuredClone(log.slice(after)), count: log.length };
  }

  async pushEvents(id: string, events: EventRecord[]): Promise<{ results: PushResult[] }> {
    this.#check();
    const log = this.logs.get(id) ?? [];
    const results = events.map((record): PushResult => {
      const eventId = record.event_id ?? null;
      if (!eventId) return { event_id: null, outcome: "rejected", reason: "no event_id" };
      const known = [...log, ...this.conflicts].some((r) => r.event_id === eventId);
      if (known) return { event_id: eventId, outcome: "duplicate" };
      try {
        core.summarize([...log, record]);
        log.push(structuredClone(record));
        return { event_id: eventId, outcome: "recorded" };
      } catch (error) {
        this.conflicts.push(record);
        return { event_id: eventId, outcome: "conflict", reason: String(error) };
      }
    });
    return { results };
  }

  async registerExecution(
    id: string,
    _procedure: string,
    _t: ProcedureTemplate,
    events: EventRecord[],
  ) {
    this.#check();
    if (!this.logs.has(id)) {
      const logMeta = {
        type: "log_meta",
        at: new Date().toISOString(),
        version: 2,
        tool_version: "test",
      };
      this.logs.set(id, [logMeta as EventRecord, ...structuredClone(events)]);
    }
    return core.summarize(this.logs.get(id) ?? []);
  }

  async getTemplate() {
    this.#check();
    return TEMPLATE;
  }

  async getSharedTemplate() {
    this.#check();
    return TEMPLATE;
  }

  /** Someone else records on the server directly. */
  record(id: string, action: Parameters<typeof core.decide>[1]) {
    const log = this.logs.get(id) ?? [];
    for (const event of core.decide(log, action))
      log.push({ ...event, event_id: crypto.randomUUID() });
  }
}

/** The plain server API, which is only reachable when the fake server is. */
function onlineApi(server: FakeServer): ExecutionApi {
  return {
    startExecution: async () => {
      throw new TypeError("Failed to fetch");
    },
    startSharedExecution: async () => {
      throw new TypeError("Failed to fetch");
    },
    getExecutionState: async (id) => core.summarize((await server.pullEvents(id, 0)).records),
    recordAction: async () => {
      throw new TypeError("Failed to fetch");
    },
    watchExecution: () => () => {},
  };
}

function setup() {
  const server = new FakeServer();
  const local = new LocalExecutions(memoryStore(), server, onlineApi(server), realCore);
  return { server, local };
}

const meta = () => ({ event_id: crypto.randomUUID(), recorded_by: "tester", device: "tablet" });
const check = (index: number, checked = true) => ({
  action: "toggle_checkbox" as const,
  step_id: "step-0",
  checkbox_id: `step-0/cb-${index}`,
  checked,
});
const checked = (summary: ExecutionSummary) =>
  summary.steps[0]?.content.map((item) => item.type === "Checkbox" && item.checked);

describe("local-first executions", () => {
  test("start offline from a kept template, record, and sync when back", async () => {
    const { server, local } = setup();
    await local.keepTemplates([PATH]);
    server.reachable = false;

    const started = await local.startExecution(PATH, meta());
    const recorded = await local.recordAction(started.execution_id, check(0), meta());

    expect(checked(recorded)).toEqual([true, false]);
    expect(await local.waitingCount()).toBeGreaterThan(0);
    await expect(local.sync(started.execution_id)).rejects.toBeInstanceOf(TypeError);

    server.reachable = true;
    await local.sync(started.execution_id);

    expect(await local.waitingCount()).toBe(0);
    const onServer = core.summarize(server.logs.get(started.execution_id) ?? []);
    expect(checked(onServer)).toEqual([true, false]);
    expect(server.logs.get(started.execution_id)?.[0]?.type).toBe("log_meta");
    expect(server.logs.get(started.execution_id)?.at(-1)?.recorded_by).toBe("tester");
  });

  test("an event that no longer applies is left to the server's conflicts", async () => {
    const { server, local } = setup();
    await local.keepTemplates([PATH]);
    server.reachable = false;
    const { execution_id: id } = await local.startExecution(PATH, meta());
    server.reachable = true;
    await local.sync(id);

    server.reachable = false;
    await local.recordAction(id, check(0), meta());
    // Meanwhile someone completes the execution on the server.
    server.record(id, { action: "complete", status: "pass" });
    const summaries: ExecutionSummary[] = [];
    const stop = local.watchExecution(
      id,
      () => {},
      (summary) => summaries.push(summary),
    );
    server.reachable = true;
    await local.sync(id);
    stop();

    expect(server.conflicts).toHaveLength(1);
    expect(await local.waitingCount()).toBe(0);
    const after = summaries.at(-1);
    expect(after?.status).toBe("pass");
    expect(checked(after as ExecutionSummary)).toEqual([false, false]);
  });

  test("a retried push after a lost response doesn't record twice", async () => {
    const { server, local } = setup();
    await local.keepTemplates([PATH]);
    server.reachable = false;
    const { execution_id: id } = await local.startExecution(PATH, meta());
    server.reachable = true;
    await local.sync(id);
    await local.recordAction(id, check(1), meta());
    await local.sync(id);
    const before = server.logs.get(id)?.length;

    // Push the same events again, as after a response that never arrived.
    const event = server.logs.get(id)?.at(-1) as EventRecord;
    const again = await server.pushEvents(id, [event]);

    expect(again.results[0]?.outcome).toBe("duplicate");
    expect(server.logs.get(id)?.length).toBe(before);
  });

  test("an execution opened online can be continued offline", async () => {
    const { server, local } = setup();
    const records = core.start(TEMPLATE).map((event) => ({ ...event }));
    const id = (records[0] as { execution_id: string }).execution_id;
    await server.registerExecution(id, "p", TEMPLATE, records);

    await local.getExecutionState(id);
    server.reachable = false;
    const offline = await local.getExecutionState(id);
    const recorded = await local.recordAction(id, check(1), meta());

    expect(offline.execution_id).toBe(id);
    expect(checked(recorded)).toEqual([false, true]);
    expect((await local.summaries()).map((s) => s.execution_id)).toContain(id);
  });

  test("an execution never opened here can't be shown offline", async () => {
    const { server, local } = setup();
    server.reachable = false;

    await expect(local.getExecutionState("unknown")).rejects.toBeInstanceOf(TypeError);
  });
});
