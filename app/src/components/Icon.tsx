// Hand-drawn 16px symbolic icons (1.5px stroke, currentColor), GNOME-symbolic in spirit.

import type { JSX } from 'react';

const paths: Record<string, JSX.Element> = {
  search: (
    <>
      <circle cx="7" cy="7" r="4.25" />
      <path d="m10.25 10.25 3.25 3.25" />
    </>
  ),
  file: (
    <>
      <path d="M4 2.75h5l3 3v7.5H4z" />
      <path d="M9 2.75v3h3" />
    </>
  ),
  code: (
    <>
      <path d="M4 2.75h5l3 3v7.5H4z" />
      <path d="m7 8.25-1.25 1.25L7 10.75M9 8.25l1.25 1.25L9 10.75" />
    </>
  ),
  globe: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M2.5 8h11M8 2.5c1.6 1.6 2.3 3.6 2.3 5.5S9.6 11.9 8 13.5C6.4 11.9 5.7 9.9 5.7 8S6.4 4.1 8 2.5Z" />
    </>
  ),
  tab: (
    <>
      <rect x="2.25" y="3.25" width="11.5" height="9.5" rx="1.5" />
      <path d="M2.25 6.25h11.5M5 3.25v3" />
    </>
  ),
  terminal: (
    <>
      <rect x="2.25" y="3.25" width="11.5" height="9.5" rx="1.5" />
      <path d="m5 6.5 1.75 1.5L5 9.5M8.25 10h2.75" />
    </>
  ),
  play: <path d="M5.25 3.6v8.8a.4.4 0 0 0 .6.35l7.1-4.4a.4.4 0 0 0 0-.7l-7.1-4.4a.4.4 0 0 0-.6.35Z" />,
  resume: (
    <>
      <path d="M3 8a5 5 0 1 0 1.5-3.55" />
      <path d="M3 3v2.5h2.5" />
      <path d="m7 6 2.5 2L7 10z" />
    </>
  ),
  review: (
    <>
      <path d="M2.75 4h7.5M2.75 8h5M2.75 12h4" />
      <path d="m9.5 11 1.5 1.5 3-3.25" />
    </>
  ),
  eye: (
    <>
      <path d="M1.75 8S4 3.75 8 3.75 14.25 8 14.25 8 12 12.25 8 12.25 1.75 8 1.75 8Z" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  copy: (
    <>
      <rect x="5.25" y="5.25" width="8" height="8" rx="1.5" />
      <path d="M10.75 5.25V3.75a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h1.5" />
    </>
  ),
  markdown: (
    <>
      <rect x="1.75" y="3.75" width="12.5" height="8.5" rx="1.5" />
      <path d="M4.25 10V6l1.75 2 1.75-2v4M10.75 6v4m-1.5-1.5 1.5 1.5 1.5-1.5" />
    </>
  ),
  trash: (
    <>
      <path d="M2.75 4.25h10.5M6.25 4.25V2.75h3.5v1.5M4 4.25l.6 8.2a1 1 0 0 0 1 .8h4.8a1 1 0 0 0 1-.8l.6-8.2" />
    </>
  ),
  pause: <path d="M5.75 3.5v9M10.25 3.5v9" />,
  clock: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5v3.25l2 1.25" />
    </>
  ),
  today: (
    <>
      <rect x="2.25" y="3.25" width="11.5" height="10" rx="1.5" />
      <path d="M2.25 6.5h11.5M5.25 2v2.5M10.75 2v2.5" />
      <path d="M5.5 9.25h2" />
    </>
  ),
  timeline: (
    <>
      <path d="M2.5 4.5h6M5 8h7.5M3.5 11.5h5" />
    </>
  ),
  settings: (
    <>
      <path d="M2.75 4.5h6.5M12.25 4.5h1M2.75 11.5h1M6.75 11.5h6.5" />
      <circle cx="10.75" cy="4.5" r="1.5" />
      <circle cx="5.25" cy="11.5" r="1.5" />
    </>
  ),
  ask: (
    <>
      <path d="M2.75 4.25a1.5 1.5 0 0 1 1.5-1.5h7.5a1.5 1.5 0 0 1 1.5 1.5v5.5a1.5 1.5 0 0 1-1.5 1.5H7l-3 2.5v-2.5h0a1.5 1.5 0 0 1-1.25-1.5z" />
      <path d="M6.6 5.6a1.45 1.45 0 0 1 2.8.45c0 .95-1.4 1.2-1.4 1.95M8 9.15v.05" />
    </>
  ),
  command: <path d="M6 6V4.5A1.5 1.5 0 1 0 4.5 6H6Zm0 0h4m-4 0v4m4-4V4.5A1.5 1.5 0 1 1 11.5 6H10Zm0 0v4m0 0h1.5a1.5 1.5 0 1 1-1.5 1.5V10Zm0 0H6m0 0v1.5A1.5 1.5 0 1 1 4.5 10H6Z" />,
  'chevron-right': <path d="m6.5 4 4 4-4 4" />,
  'chevron-left': <path d="m9.5 4-4 4 4 4" />,
  'chevron-down': <path d="m4 6.5 4 4 4-4" />,
  check: <path d="m3.5 8.5 3 3 6-7" />,
  x: <path d="m4 4 8 8M12 4l-8 8" />,
  plus: <path d="M8 3v10M3 8h10" />,
  minus: <path d="M3 8h10" />,
  refresh: (
    <>
      <path d="M13 8a5 5 0 1 1-1.5-3.55" />
      <path d="M13 3v2.5h-2.5" />
    </>
  ),
  folder: <path d="M2.25 4.5a1 1 0 0 1 1-1h3l1.5 1.5h5a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-9.5a1 1 0 0 1-1-1z" />,
  external: (
    <>
      <path d="M9 2.75h4.25V7M13.25 2.75 7.5 8.5" />
      <path d="M11.25 9.5v3a.75.75 0 0 1-.75.75h-7a.75.75 0 0 1-.75-.75v-7a.75.75 0 0 1 .75-.75h3" />
    </>
  ),
  info: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 7.25v3.5M8 5.25v.05" />
    </>
  ),
  alert: (
    <>
      <path d="M8 2.75 14 13H2z" />
      <path d="M8 6.75v3M8 11.25v.05" />
    </>
  ),
  shield: <path d="M8 2.25 3 4v4c0 3 2.2 5 5 5.75 2.8-.75 5-2.75 5-5.75V4z" />,
  chip: (
    <>
      <rect x="4" y="4" width="8" height="8" rx="1.25" />
      <path d="M6 1.75V4M10 1.75V4M6 12v2.25M10 12v2.25M1.75 6H4M1.75 10H4M12 6h2.25M12 10h2.25" />
    </>
  ),
  database: (
    <>
      <ellipse cx="8" cy="4" rx="5" ry="1.75" />
      <path d="M3 4v8c0 1 2.25 1.75 5 1.75s5-.75 5-1.75V4M3 8c0 1 2.25 1.75 5 1.75S13 9 13 8" />
    </>
  ),
  keyboard: (
    <>
      <rect x="1.75" y="4" width="12.5" height="8" rx="1.5" />
      <path d="M4.5 6.75h.01M7 6.75h.01M9.5 6.75h.01M12 6.75h.01M5 9.5h6" />
    </>
  ),
  sliders: (
    <>
      <path d="M2.75 4.5h6.5M12.25 4.5h1M2.75 11.5h1M6.75 11.5h6.5" />
      <circle cx="10.75" cy="4.5" r="1.5" />
      <circle cx="5.25" cy="11.5" r="1.5" />
    </>
  ),
  capture: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  layers: (
    <>
      <path d="m8 2.75 5.5 3L8 8.75l-5.5-3z" />
      <path d="m2.5 9 5.5 3 5.5-3" />
    </>
  ),
  todo: (
    <>
      <rect x="2.75" y="2.75" width="10.5" height="10.5" rx="2" />
      <path d="M5.5 8h5" />
    </>
  ),
  arrow: <path d="M3 8h10m-4-4 4 4-4 4" />,
  enter: <path d="M12.5 3.5V8a1.5 1.5 0 0 1-1.5 1.5H4m2.5-2.5L4 9.5 6.5 12" />,
  dot: <circle cx="8" cy="8" r="2" fill="currentColor" stroke="none" />,
  calendar: (
    <>
      <rect x="2.25" y="3.25" width="11.5" height="10" rx="1.5" />
      <path d="M2.25 6.5h11.5M5.25 2v2.5M10.75 2v2.5" />
    </>
  ),
  at: (
    <>
      <circle cx="8" cy="8" r="2.25" />
      <path d="M10.25 8v1a1.75 1.75 0 0 0 3.5 0V8a5.75 5.75 0 1 0-2.3 4.6" />
    </>
  ),
  bolt: <path d="M9 1.75 3.75 9H8l-1 5.25L12.25 7H8z" />,
  lock: (
    <>
      <rect x="3.25" y="7" width="9.5" height="6.75" rx="1.5" />
      <path d="M5.25 7V5a2.75 2.75 0 0 1 5.5 0v2" />
    </>
  ),
};

export type IconName = keyof typeof paths;

export function Icon({ name, size = 16, className }: { name: IconName; size?: number; className?: string }) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {paths[name]}
    </svg>
  );
}
