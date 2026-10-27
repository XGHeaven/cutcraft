import test from "node:test";
import assert from "node:assert/strict";
import { ApiError, request } from "../src/api.ts";

test("semantic failures in successful HTTP responses reject and preserve error details", async (t) => {
  const original = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = original;
  });
  globalThis.fetch = async () =>
    new Response(
      '{"ok":false,"error":{"code":"INVALID_COMMAND","message":"Cannot trim this item."}}',
      { status: 200 },
    );
  await assert.rejects(
    request("/projects/p/commands", {}),
    (e) =>
      e instanceof ApiError &&
      e.status === 400 &&
      e.message === "Cannot trim this item." &&
      e.details.error.code === "INVALID_COMMAND",
  );
  globalThis.fetch = async () =>
    new Response(
      '{"ok":false,"error":{"code":"REVISION_CONFLICT","message":"Project changed."},"revision":9007199254740993}',
      { status: 200 },
    );
  await assert.rejects(
    request("/projects/p/history/undo", {}),
    (e) =>
      e instanceof ApiError &&
      e.status === 409 &&
      e.details.revision === 9007199254740993n,
  );
  globalThis.fetch = async () =>
    new Response(
      '{"error":{"code":"PROJECT_BUSY","message":"Export is running."}}',
      { status: 409 },
    );
  await assert.rejects(
    request("/projects/p/close", {}),
    (e) => e instanceof ApiError && e.status === 409,
  );
  globalThis.fetch = async () =>
    new Response('{"ok":true,"revision":9007199254740993}', { status: 200 });
  assert.deepEqual(await request("/projects/p/commands", {}), {
    ok: true,
    revision: 9007199254740993n,
  });
});

test("request IDs work without secure-context randomUUID on LAN HTTP", async (t) => {
  const { requestId } = await import("../src/api.ts");
  const original = globalThis.crypto;
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: { getRandomValues: original.getRandomValues.bind(original) },
  });
  t.after(() =>
    Object.defineProperty(globalThis, "crypto", {
      configurable: true,
      value: original,
    }),
  );
  const ids = new Set(Array.from({ length: 100 }, requestId));
  assert.equal(ids.size, 100);
  for (const id of ids)
    assert.match(
      id,
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
});
