/** Exact editor times. Numbers are used only for pixel layout and playback display. */
export type Rational = { num: bigint; den: bigint };
const abs = (n: bigint) => (n < 0n ? -n : n);
export function rational(num: bigint, den = 1n): Rational {
  if (!den) throw new Error("A time denominator cannot be zero.");
  if (den < 0n) {
    num = -num;
    den = -den;
  }
  let a = abs(num),
    b = den;
  while (b) [a, b] = [b, a % b];
  return { num: num / (a || 1n), den: den / (a || 1n) };
}
export const zero = rational(0n);
export const from = (v: {
  num: bigint | number | string;
  den: bigint | number | string;
}): Rational => rational(BigInt(v.num), BigInt(v.den));
export const add = (a: Rational, b: Rational) =>
  rational(a.num * b.den + b.num * a.den, a.den * b.den);
export const sub = (a: Rational, b: Rational) =>
  rational(a.num * b.den - b.num * a.den, a.den * b.den);
export const mul = (a: Rational, b: Rational) =>
  rational(a.num * b.num, a.den * b.den);
export const div = (a: Rational, b: Rational) =>
  rational(a.num * b.den, a.den * b.num);
export const compare = (a: Rational, b: Rational) =>
  a.num * b.den < b.num * a.den ? -1 : a.num * b.den > b.num * a.den ? 1 : 0;
export const max = (a: Rational, b: Rational) => (compare(a, b) > 0 ? a : b);
export const min = (a: Rational, b: Rational) => (compare(a, b) < 0 ? a : b);
export const seconds = (a: Rational) => Number(a.num) / Number(a.den);
export const exact = (a: Rational) =>
  a.den === 1n ? String(a.num) : `${a.num}/${a.den}`;
export function parseTime(value: string): Rational {
  const s = value.trim();
  if (/^[+-]?\d+\/[1-9]\d*$/.test(s)) {
    const [n, d] = s.split("/");
    return rational(BigInt(n), BigInt(d));
  }
  if (!/^[+-]?\d+(\.\d+)?$/.test(s))
    throw new Error(
      "Enter seconds (e.g. 2.5) or an exact fraction (e.g. 1001/30000).",
    );
  const negative = s.startsWith("-");
  const [whole, decimal = ""] = s.replace(/^[+-]/, "").split(".");
  return rational(
    (negative ? -1n : 1n) * BigInt(whole + decimal),
    10n ** BigInt(decimal.length),
  );
}
export function nearestFrame(t: Rational, fps: Rational): bigint {
  const n = t.num * fps.num,
    d = t.den * fps.den;
  return n >= 0n ? (n + d / 2n) / d : -((-n + d / 2n) / d);
}
export const frameTime = (frame: bigint, fps: Rational) =>
  rational(frame * fps.den, fps.num);
export const snap = (t: Rational, fps: Rational) =>
  frameTime(nearestFrame(t, fps), fps);
/** Pointer coordinates become an integer frame index; no float-second round trip. */
export const pixelTime = (
  pixels: number,
  pixelsPerSecond: number,
  fps: Rational,
) =>
  frameTime(BigInt(Math.round((pixels / pixelsPerSecond) * seconds(fps))), fps);
export const display = (t: Rational) =>
  `${Math.floor(seconds(t) / 60)}:${(seconds(t) % 60).toFixed(3).padStart(6, "0")}`;

/** The browser clock selects a frame index, never a persisted floating-point time. */
export function playbackPosition(
  clockSeconds: number,
  fps: Rational,
  origin: Rational,
  end: Rational,
): Rational {
  const count = mul(sub(end, origin), fps);
  const last = (count.num + count.den - 1n) / count.den - 1n;
  if (last < 0n) return origin;
  const approximateFrame = Number.isFinite(clockSeconds)
    ? Math.max(
        0,
        Math.round((clockSeconds * Number(fps.num)) / Number(fps.den)),
      )
    : 0;
  const index = BigInt(approximateFrame);
  return add(origin, frameTime(index > last ? last : index, fps));
}
