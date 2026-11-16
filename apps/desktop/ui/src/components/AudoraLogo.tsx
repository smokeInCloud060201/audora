import React, { useId } from "react";

interface AudoraLogoProps {
  /** Pixel size of the rendered SVG. */
  size?: number;
  /**
   * "mark" renders the compact mint rounded-square logo mark (`12:69` / `12:89`) for headers/lockups.
   * "icon" renders the full Listening Aperture app icon (`12:9` / `12:45`) for dialogs & About cards.
   */
  variant?: "mark" | "icon";
  /** Animates the logo mark & equalizer bars when media is actively playing (e.g., in Minimum Mode). */
  isPlaying?: boolean;
  className?: string;
}

/**
 * Official Audora Brand System assets from Figma ("Audora — App icon & logo", frame `12:2`):
 * - `App Icon / Master / Dark` (`12:9`) & `32px` responsive optical size (`12:45`)
 * - `Audora logo mark` (`12:69` / `12:89`)
 */
export const AudoraLogo: React.FC<AudoraLogoProps> = ({
  size = 28,
  variant = "mark",
  isPlaying = false,
  className = "",
}) => {
  const uid = useId().replace(/:/g, "");

  if (variant === "mark") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 62 62"
        fill="none"
        xmlns="http://www.w3.org/2000/svg"
        className={`audora-logo-svg ${isPlaying ? "is-playing" : ""} ${className}`.trim()}
        aria-hidden="true"
        data-node-id="12:89"
      >
        <rect className="audora-mark-bg" width="62" height="62" rx="16" fill="#79D7BF" />
        <rect className="audora-mark-bar bar-1" x="9" y="26.5" width="4" height="9" rx="2" fill="#10201C" />
        <rect className="audora-mark-bar bar-2" x="19" y="22.36" width="4" height="17.28" rx="2" fill="#10201C" />
        <rect className="audora-mark-bar bar-3" x="29" y="18.04" width="4" height="25.92" rx="2" fill="#10201C" />
        <rect className="audora-mark-bar bar-4" x="39" y="21.46" width="4" height="19.08" rx="2" fill="#10201C" />
        <rect className="audora-mark-bar bar-5" x="49" y="25.24" width="4" height="11.52" rx="2" fill="#10201C" />
      </svg>
    );
  }

  if (size <= 36) {
    const clipId = `audora-clip-32-${uid}`;
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 32 32"
        fill="none"
        xmlns="http://www.w3.org/2000/svg"
        className={`audora-logo-svg ${className}`.trim()}
        aria-hidden="true"
        data-node-id="12:45"
      >
        <defs>
          <clipPath id={clipId}>
            <rect width="32" height="32" rx="7.04" />
          </clipPath>
        </defs>
        <g clipPath={`url(#${clipId})`}>
          <rect width="32" height="32" rx="7.04" fill="#101816" />
          <circle cx="16" cy="16" r="10.24" fill="#79D7BF" />
          <rect x="8.48" y="13.44" width="1.344" height="5.12" rx="0.672" fill="#101816" />
          <rect x="11.904" y="11.52" width="1.344" height="8.96" rx="0.672" fill="#101816" />
          <rect x="15.328" y="9.6" width="1.344" height="12.8" rx="0.672" fill="#101816" />
          <rect x="18.752" y="11.04" width="1.344" height="9.92" rx="0.672" fill="#101816" />
          <rect x="22.176" y="12.8" width="1.344" height="6.4" rx="0.672" fill="#101816" />
          <circle cx="22.576" cy="9.456" r="1.2" fill="#101816" />
          <rect x="0.5" y="0.5" width="31" height="31" rx="6.54" stroke="#FFFFFF" strokeOpacity="0.08" />
        </g>
      </svg>
    );
  }

  const clipId = `audora-clip-1024-${uid}`;
  const glowId = `audora-glow-1024-${uid}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 1024 1024"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={`audora-logo-svg ${className}`.trim()}
      aria-hidden="true"
      data-node-id="12:9"
    >
      <defs>
        <clipPath id={clipId}>
          <rect width="1024" height="1024" rx="225.28" />
        </clipPath>
        <filter
          id={glowId}
          x="61.44"
          y="102.4"
          width="901.12"
          height="901.12"
          filterUnits="userSpaceOnUse"
          colorInterpolationFilters="sRGB"
        >
          <feFlood floodOpacity="0" result="BackgroundImageFix" />
          <feColorMatrix
            in="SourceAlpha"
            type="matrix"
            values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 127 0"
            result="hardAlpha"
          />
          <feOffset dy="40.96" />
          <feGaussianBlur stdDeviation="61.44" />
          <feColorMatrix
            type="matrix"
            values="0 0 0 0 0.47451 0 0 0 0 0.843137 0 0 0 0 0.74902 0 0 0 0.18 0"
          />
          <feBlend mode="normal" in2="BackgroundImageFix" result="dropShadow" />
          <feBlend mode="normal" in="SourceGraphic" in2="dropShadow" result="shape" />
        </filter>
      </defs>
      <g clipPath={`url(#${clipId})`}>
        <rect width="1024" height="1024" rx="225.28" fill="#101816" />
        <g filter={`url(#${glowId})`}>
          <circle cx="512" cy="512" r="327.68" fill="#79D7BF" />
        </g>
        <rect x="271.36" y="430.08" width="43.008" height="163.84" rx="21.504" fill="#101816" />
        <rect x="380.928" y="368.64" width="43.008" height="286.72" rx="21.504" fill="#101816" />
        <rect x="490.496" y="307.2" width="43.008" height="409.6" rx="21.504" fill="#101816" />
        <rect x="600.064" y="353.28" width="43.008" height="317.44" rx="21.504" fill="#101816" />
        <rect x="709.632" y="409.6" width="43.008" height="204.8" rx="21.504" fill="#101816" />
        <circle cx="722.432" cy="302.592" r="38.4" fill="#101816" />
        <rect
          x="2.048"
          y="2.048"
          width="1019.904"
          height="1019.904"
          rx="223.232"
          stroke="#FFFFFF"
          strokeOpacity="0.08"
          strokeWidth="4.096"
        />
      </g>
    </svg>
  );
};
