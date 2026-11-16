import React from "react";

export interface IconProps {
  size?: number;
  className?: string;
  style?: React.CSSProperties;
}

function baseSvgProps(
  size: number,
  className?: string,
  style?: React.CSSProperties,
): React.SVGProps<SVGSVGElement> {
  return {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.85,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    "aria-hidden": true,
    focusable: "false",
    className,
    style: { flexShrink: 0, display: "inline-block", verticalAlign: "middle", ...style },
  };
}

export function IconFolderOpen({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2.5h7.5A2.5 2.5 0 0 1 21 10v7.5A2.5 2.5 0 0 1 18.5 20h-13A2.5 2.5 0 0 1 3 17.5V7.5Z" />
      <path d="M3 11h18" />
    </svg>
  );
}

export function IconMinimizeMode({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <rect x="12" y="11.5" width="6.5" height="5.5" rx="1.2" fill="currentColor" stroke="none" />
      <path d="M7 8l3.5 3.5" />
      <path d="M10.5 8.5v3h-3" />
    </svg>
  );
}

export function IconExpandMode({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M14 4h6v6" />
      <path d="M20 4l-7.5 7.5" />
      <path d="M10 20H4v-6" />
      <path d="M4 20l7.5-7.5" />
    </svg>
  );
}

export function IconHelpCircle({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <circle cx="12" cy="12" r="9" />
      <path d="M9.6 9.2a2.5 2.5 0 0 1 4.7 1.1c0 1.6-2.3 2.1-2.3 3.4" />
      <circle cx="12" cy="17.1" r="0.9" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function IconLibrary({ size = 18, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <rect x="3.5" y="4" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="4" width="7" height="7" rx="1.5" />
      <rect x="3.5" y="13" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="13" width="7" height="7" rx="1.5" />
    </svg>
  );
}

export function IconPlay({ size = 16, className, style }: IconProps): React.ReactElement {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
      className={className}
      style={{ flexShrink: 0, display: "inline-block", verticalAlign: "middle", ...style }}
    >
      <path d="M7.5 5.4c0-1.05 1.16-1.69 2.05-1.13l10.2 6.6a1.35 1.35 0 0 1 0 2.26l-10.2 6.6c-.89.56-2.05-.08-2.05-1.13V5.4Z" />
    </svg>
  );
}

export function IconPause({ size = 16, className, style }: IconProps): React.ReactElement {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
      className={className}
      style={{ flexShrink: 0, display: "inline-block", verticalAlign: "middle", ...style }}
    >
      <rect x="6.5" y="4.5" width="4" height="15" rx="1.3" />
      <rect x="13.5" y="4.5" width="4" height="15" rx="1.3" />
    </svg>
  );
}

export function IconSkipPrev({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M6 5v14" />
      <polygon points="18 5 9.5 12 18 19 18 5" fill="currentColor" />
    </svg>
  );
}

export function IconSkipNext({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M18 5v14" />
      <polygon points="6 5 14.5 12 6 19 6 5" fill="currentColor" />
    </svg>
  );
}

export function IconVolume({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <polygon points="11 5 6 9 3 9 3 15 6 15 11 19 11 5" />
      <path d="M15.2 8.8a4.5 4.5 0 0 1 0 6.4" />
      <path d="M18.2 6a8.5 8.5 0 0 1 0 12" />
    </svg>
  );
}

export function IconVolumeMuted({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <polygon points="11 5 6 9 3 9 3 15 6 15 11 19 11 5" />
      <path d="M21 9l-5 6" />
      <path d="M16 9l5 6" />
    </svg>
  );
}

export function IconPlaylists({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M4 6h11" />
      <path d="M4 11h11" />
      <path d="M4 16h7" />
      <path d="M17 11v6.5a2 2 0 1 1-1.5-1.94V9l4-1v3" />
    </svg>
  );
}

export function IconPanelRight({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <path d="M15 4v16" />
    </svg>
  );
}

export function IconCollapseLeft({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <path d="M9 4v16" />
      <path d="M16 9.5L13.5 12 16 14.5" />
    </svg>
  );
}

export function IconCollapseRight({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <path d="M15 4v16" />
      <path d="M8 9.5L10.5 12 8 14.5" />
    </svg>
  );
}

export function IconChevronLeft({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M15 18l-6-6 6-6" />
    </svg>
  );
}

export function IconArrowRight({ size = 13, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M5 12h14" />
      <path d="M13 6l6 6-6 6" />
    </svg>
  );
}

export function IconArrowUp({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M12 19V5" />
      <path d="M6 11l6-6 6 6" />
    </svg>
  );
}

export function IconArrowDown({ size = 15, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M12 5v14" />
      <path d="M6 13l6 6 6-6" />
    </svg>
  );
}

export function IconSettings({ size = 18, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <circle cx="12" cy="12" r="3.2" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.08a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.08a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.08a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
    </svg>
  );
}

export function IconPlus({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </svg>
  );
}

export function IconFlag({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M5 21V4" />
      <path d="M5 4h11.5l-2.2 4 2.2 4H5" />
    </svg>
  );
}

export function IconMoreHorizontal({
  size = 16,
  className,
  style,
}: IconProps): React.ReactElement {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
      className={className}
      style={{ flexShrink: 0, display: "inline-block", verticalAlign: "middle", ...style }}
    >
      <circle cx="5.5" cy="12" r="1.8" />
      <circle cx="12" cy="12" r="1.8" />
      <circle cx="18.5" cy="12" r="1.8" />
    </svg>
  );
}

export function IconShuffle({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M16 3h5v5" />
      <path d="M4 20L21 3" />
      <path d="M21 16v5h-5" />
      <path d="M15 15l6 6" />
      <path d="M4 4l5 5" />
    </svg>
  );
}

export function IconRepeat({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M17 2l4 4-4 4" />
      <path d="M3 11V9a3 3 0 0 1 3-3h15" />
      <path d="M7 22l-4-4 4-4" />
      <path d="M21 13v2a3 3 0 0 1-3 3H3" />
    </svg>
  );
}

export function IconClose({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M18 6L6 18" />
      <path d="M6 6l12 12" />
    </svg>
  );
}

export function IconCheck({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M20 6L9 17l-5-5" />
    </svg>
  );
}

export function IconAlertCircle({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 8v4.5" />
      <circle cx="12" cy="16" r="0.9" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function IconEdit({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5Z" />
    </svg>
  );
}

export function IconTrash({ size = 14, className, style }: IconProps): React.ReactElement {
  return (
    <svg {...baseSvgProps(size, className, style)}>
      <path d="M3 6h18" />
      <path d="M8 6V4.5A1.5 1.5 0 0 1 9.5 3h5A1.5 1.5 0 0 1 16 4.5V6" />
      <path d="M19 6l-1 13.5a2 2 0 0 1-2 1.8H8a2 2 0 0 1-2-1.8L5 6" />
      <path d="M10 11v5.5" />
      <path d="M14 11v5.5" />
    </svg>
  );
}
