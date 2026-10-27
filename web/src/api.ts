import { LosslessNumber, parse, stringify } from "lossless-json";
export const decode = (text: string): any =>
  parse(text, undefined, {
    parseNumber: (s: string) =>
      /^-?\d+$/.test(s) ? BigInt(s) : new LosslessNumber(s),
  });
export const encode = (value: unknown): string => stringify(value) ?? "null";
export class ApiError extends Error {
  readonly status: number;
  readonly details: any;
  constructor(message: string, status: number, details: any) {
    super(message);
    this.status = status;
    this.details = details;
  }
}
export async function request<T = any>(
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : encode(body),
    signal,
  });
  const text = await response.text();
  let data: any;
  try {
    data = text ? decode(text) : null;
  } catch {
    throw new ApiError(
      `Server returned an invalid response (${response.status}).`,
      response.status,
      null,
    );
  }
  if (!response.ok || data?.ok === false) {
    const status =
      data?.error?.code === "REVISION_CONFLICT"
        ? 409
        : response.ok
          ? 400
          : response.status;
    throw new ApiError(
      data?.error?.message ??
        data?.message ??
        (typeof data?.error === "string"
          ? data.error
          : `Request failed (${response.status})`),
      status,
      data,
    );
  }
  return data as T;
}
export async function authenticate(): Promise<void> {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const code = fragment.get("code");
  if (code) {
    const search = new URLSearchParams(location.search);
    const project = fragment.get("project");
    if (project) search.set("project", project);
    history.replaceState(
      null,
      "",
      location.pathname + (search.size ? `?${search}` : ""),
    );
    await request("/session", { code });
  }
}
// getRandomValues also works on LAN HTTP, where randomUUID is unavailable.
export function requestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 0x40;
  bytes[8] = (bytes[8] & 63) | 0x80;
  const hex = [...bytes].map((v) => v.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
/** UUIDv7 is required for persistent domain IDs; randomUUID alone is v4. */
export function entityId(prefix: string): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  let ms = BigInt(Date.now());
  for (let i = 5; i >= 0; i--) {
    b[i] = Number(ms & 255n);
    ms >>= 8n;
  }
  b[6] = (b[6] & 15) | 0x70;
  b[8] = (b[8] & 63) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${prefix}_${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
