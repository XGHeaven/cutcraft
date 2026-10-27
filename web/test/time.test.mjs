import test from "node:test";
import assert from "node:assert/strict";
import * as t from "../src/time.ts";
import { decode, encode, entityId } from "../src/api.ts";
test("canonical arithmetic and fractional frame positions stay exact", () => {
  const fps = t.rational(30000n, 1001n);
  assert.deepEqual(t.parseTime("0.125"), { num: 1n, den: 8n });
  assert.deepEqual(t.rational(0n, 999n), t.zero);
  assert.equal(
    t.exact(t.frameTime(9007199254740993n, fps)),
    "3005402151331911331/10000",
  );
  assert.equal(
    t.nearestFrame(t.frameTime(9007199254740993n, fps), fps),
    9007199254740993n,
  );
  assert.deepEqual(t.snap(t.parseTime("1"), fps), t.rational(1001n, 1000n));
  assert.deepEqual(
    t.add(t.parseTime("1/3"), t.parseTime("1/6")),
    t.parseTime("1/2"),
  );
  assert.deepEqual(t.parseTime("-1.25"), t.rational(-5n, 4n));
  assert.throws(() => t.parseTime("1/0"));
  assert.throws(() => t.parseTime("NaN"));
});
test("snapshot integer limits survive read, clone and JSON submit", () => {
  const source =
    '{"revision":9007199254740993,"time":{"num":9223372036854775807,"den":30000},"opacity":0.5,"opaque":{"large":18446744073709551615,"precise":0.123456789012345678901,"exponent":1.234567890123456789e30}}';
  const parsed = decode(source);
  assert.equal(parsed.revision, 9007199254740993n);
  assert.equal(parsed.time.num, 9223372036854775807n);
  assert.equal(encode(parsed), source);
  assert.equal(
    encode({
      ...parsed,
      time: { num: -9223372036854775808n, den: 1n },
    }).includes('"num":-9223372036854775808'),
    true,
  );
});
test("new persistent ids are typed UUIDv7", () => {
  assert.match(
    entityId("item"),
    /^item_[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  assert.notEqual(entityId("track"), entityId("track"));
});

test("pause and end positions select integer preview frames without float-time writes", () => {
  const fps = t.rational(30000n, 1001n),
    origin = t.rational(1n, 3n),
    end = t.rational(5n, 2n);
  assert.deepEqual(
    t.playbackPosition(1.001, fps, origin, end),
    t.add(origin, t.frameTime(30n, fps)),
  );
  assert.deepEqual(
    t.playbackPosition(100, fps, origin, end),
    t.add(origin, t.frameTime(64n, fps)),
  );
  assert.deepEqual(t.playbackPosition(-1, fps, origin, end), origin);
  assert.deepEqual(t.playbackPosition(Number.NaN, fps, origin, end), origin);
  assert.ok(t.compare(t.playbackPosition(100, fps, origin, end), end) < 0);
});
