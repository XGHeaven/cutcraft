import type { ReactNode } from "react";

const paths = {
  undo: (
    <>
      <path d="m8 5-5 5 5 5" />
      <path d="M3 10h10a7 7 0 0 1 7 7v2" />
    </>
  ),
  redo: (
    <>
      <path d="m16 5 5 5-5 5" />
      <path d="M21 10H11a7 7 0 0 0-7 7v2" />
    </>
  ),
  play: <path d="m9 5 11 7-11 7Z" fill="currentColor" stroke="none" />,
  pause: (
    <>
      <path d="M8 5v14M16 5v14" strokeWidth="3" />
    </>
  ),
  previous: (
    <>
      <path d="M5 5v14" />
      <path d="m18 5-10 7 10 7Z" />
    </>
  ),
  next: (
    <>
      <path d="M19 5v14" />
      <path d="m6 5 10 7-10 7Z" />
    </>
  ),
  add: <path d="M12 5v14M5 12h14" />,
  minus: <path d="M5 12h14" />,
  close: <path d="m6 6 12 12M18 6 6 18" />,
  chevron: <path d="m7 10 5 5 5-5" />,
  export: (
    <>
      <path d="M12 15V3m-4 4 4-4 4 4M5 14v6h14v-6" />
    </>
  ),
  folder: (
    <path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1Z" />
  ),
  split: (
    <>
      <circle cx="6" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <path d="m8 8 12 12M8 16 20 4" />
    </>
  ),
  media: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <circle cx="8" cy="9" r="1" />
      <path d="m3 17 5-4 4 3 4-6 5 7" />
    </>
  ),
  audio: <path d="M4 10v4m4-8v12m4-15v18m4-15v12m4-8v4" />,
  track: (
    <>
      <rect x="3" y="4" width="18" height="6" rx="1" />
      <rect x="3" y="14" width="12" height="6" rx="1" />
    </>
  ),
  settings: (
    <>
      <path d="M4 7h7m4 0h5M4 17h3m4 0h9" />
      <circle cx="13" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </>
  ),
  tasks: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  eye: (
    <>
      <path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  eyeOff: (
    <>
      <path d="m3 3 18 18M10 5a12 12 0 0 1 12 7s-1 3-4 5M6 6a16 16 0 0 0-4 6s3 7 10 7c2 0 3-.5 4-1M10 10a3 3 0 0 0 4 4" />
    </>
  ),
  lock: (
    <>
      <rect x="5" y="10" width="14" height="11" rx="2" />
      <path d="M8 10V7a4 4 0 0 1 8 0v3" />
    </>
  ),
  logo: (
    <>
      <path d="M18 5H9a6 6 0 0 0-6 6v2a6 6 0 0 0 6 6h5" strokeWidth="2.5" />
      <path d="m15 11 6 4-6 4Z" fill="currentColor" stroke="none" />
    </>
  ),
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof paths;
export function Icon({ name }: { name: IconName }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {paths[name]}
    </svg>
  );
}
