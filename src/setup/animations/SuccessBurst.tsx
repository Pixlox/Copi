import { useEffect, useState, useMemo } from "react";

interface BurstParticle {
  id: number;
  angle: number;
  distance: number;
  size: number;
  delay: number;
  duration: number;
}

function generateBurstParticles(count: number): BurstParticle[] {
  return Array.from({ length: count }, (_, i) => ({
    id: i,
    angle: (360 / count) * i + (Math.random() * 30 - 15), // Spread around circle with jitter
    distance: 40 + Math.random() * 60, // 40-100px from center
    size: 4 + Math.random() * 6, // 4-10px
    delay: Math.random() * 100, // 0-100ms stagger
    duration: 400 + Math.random() * 200, // 400-600ms
  }));
}

interface SuccessBurstProps {
  active: boolean;
  particleCount?: number;
  onComplete?: () => void;
}

export default function SuccessBurst({
  active,
  particleCount = 14,
  onComplete,
}: SuccessBurstProps) {
  const [isAnimating, setIsAnimating] = useState(false);
  const particles = useMemo(() => generateBurstParticles(particleCount), [particleCount]);

  useEffect(() => {
    if (active && !isAnimating) {
      setIsAnimating(true);
      const timer = setTimeout(() => {
        setIsAnimating(false);
        onComplete?.();
      }, 700);
      return () => clearTimeout(timer);
    }
  }, [active, isAnimating, onComplete]);

  if (!isAnimating) return null;

  return (
    <div className="launch-burst">
      {particles.map((p) => {
        const radians = (p.angle * Math.PI) / 180;
        const x = Math.cos(radians) * p.distance;
        const y = Math.sin(radians) * p.distance;

        return (
          <div
            key={p.id}
            className="launch-burst-particle"
            style={{
              width: `${p.size}px`,
              height: `${p.size}px`,
              "--burst-x": `${x}px`,
              "--burst-y": `${y}px`,
              animationDelay: `${p.delay}ms`,
              animationDuration: `${p.duration}ms`,
            } as React.CSSProperties}
          />
        );
      })}
      {/* Central flash */}
      <div className="launch-burst-flash" />
    </div>
  );
}
