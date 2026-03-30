import { useEffect, useState } from "react";

type AnimationPhase = "hidden" | "back-entering" | "front-entering" | "lines-appearing" | "complete";

interface LogoAssemblyProps {
  startDelay?: number;
  onComplete?: () => void;
}

export default function LogoAssembly({ startDelay = 0, onComplete }: LogoAssemblyProps) {
  const [phase, setPhase] = useState<AnimationPhase>("hidden");

  useEffect(() => {
    const timers: ReturnType<typeof setTimeout>[] = [];

    // Start the animation sequence after initial delay
    timers.push(
      setTimeout(() => setPhase("back-entering"), startDelay)
    );
    timers.push(
      setTimeout(() => setPhase("front-entering"), startDelay + 300)
    );
    timers.push(
      setTimeout(() => setPhase("lines-appearing"), startDelay + 700)
    );
    timers.push(
      setTimeout(() => {
        setPhase("complete");
        onComplete?.();
      }, startDelay + 1200)
    );

    return () => timers.forEach(clearTimeout);
  }, [startDelay, onComplete]);

  const showBack = phase !== "hidden";
  const showFront = phase !== "hidden" && phase !== "back-entering";
  const showLines = phase === "lines-appearing" || phase === "complete";
  const isComplete = phase === "complete";

  return (
    <div className="launch-logo-container">
      <svg
        className="launch-logo"
        width="120"
        height="120"
        viewBox="0 0 512 512"
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <linearGradient id="launch-bg" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#1a1a1f" />
            <stop offset="100%" stopColor="#111114" />
          </linearGradient>
          <linearGradient id="launch-front" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#ffffff" stopOpacity="1" />
            <stop offset="100%" stopColor="#c8c8d0" stopOpacity="1" />
          </linearGradient>
          <linearGradient id="launch-back" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#ffffff" stopOpacity="0.22" />
            <stop offset="100%" stopColor="#ffffff" stopOpacity="0.10" />
          </linearGradient>
          <filter id="launch-glow" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="8" result="blur" />
            <feComposite in="SourceGraphic" in2="blur" operator="over" />
          </filter>
        </defs>

        <g transform="translate(56, 56) scale(0.781)">
          {/* Background rounded rectangle */}
          <rect
            className={`launch-logo-bg ${isComplete ? "launch-logo-bg--glow" : ""}`}
            width="512"
            height="512"
            rx="112"
            ry="112"
            fill="url(#launch-bg)"
          />

          {/* Back clipboard - slides from right */}
          <rect
            className={`launch-logo-back ${showBack ? "launch-logo-back--visible" : ""}`}
            x="190"
            y="170"
            width="196"
            height="236"
            rx="28"
            ry="28"
            fill="url(#launch-back)"
            stroke="rgba(255,255,255,0.12)"
            strokeWidth="1.5"
          />

          {/* Front clipboard - slides from left */}
          <rect
            className={`launch-logo-front ${showFront ? "launch-logo-front--visible" : ""}`}
            x="158"
            y="138"
            width="196"
            height="236"
            rx="28"
            ry="28"
            fill="url(#launch-front)"
          />

          {/* Text lines - appear last */}
          <g className={`launch-logo-lines ${showLines ? "launch-logo-lines--visible" : ""}`}>
            <rect x="188" y="186" width="80" height="8" rx="4" fill="#1a1a1f" opacity="0.18" />
            <rect
              x="188"
              y="206"
              width="136"
              height="8"
              rx="4"
              fill="#1a1a1f"
              opacity="0.12"
              style={{ animationDelay: "100ms" }}
            />
            <rect
              x="188"
              y="226"
              width="112"
              height="8"
              rx="4"
              fill="#1a1a1f"
              opacity="0.12"
              style={{ animationDelay: "200ms" }}
            />
          </g>
        </g>
      </svg>

      {/* Glow ring that pulses when complete */}
      <div className={`launch-logo-glow-ring ${isComplete ? "launch-logo-glow-ring--active" : ""}`} />
    </div>
  );
}
