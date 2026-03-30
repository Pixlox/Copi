import { useMemo } from "react";

interface Particle {
  id: number;
  x: number;
  y: number;
  size: number;
  opacity: number;
  duration: number;
  delay: number;
  drift: number;
}

function generateParticles(count: number): Particle[] {
  return Array.from({ length: count }, (_, i) => ({
    id: i,
    x: Math.random() * 100,
    y: Math.random() * 100,
    size: 4 + Math.random() * 10,
    opacity: 0.15 + Math.random() * 0.35,
    duration: 20 + Math.random() * 25,
    delay: Math.random() * -20,
    drift: Math.floor(Math.random() * 3) + 1, // 1, 2, or 3 for different drift patterns
  }));
}

interface ParticlesProps {
  count?: number;
  visible?: boolean;
}

export default function Particles({ count = 22, visible = true }: ParticlesProps) {
  const particles = useMemo(() => generateParticles(count), [count]);

  return (
    <div className={`launch-particles ${visible ? "launch-particles--visible" : ""}`}>
      {particles.map((p) => (
        <div
          key={p.id}
          className={`launch-particle launch-particle--drift-${p.drift}`}
          style={{
            left: `${p.x}%`,
            top: `${p.y}%`,
            width: `${p.size}px`,
            height: `${p.size}px`,
            opacity: p.opacity,
            animationDuration: `${p.duration}s`,
            animationDelay: `${p.delay}s`,
          }}
        />
      ))}
      {/* Ambient glow in center */}
      <div className="launch-ambient-glow" />
    </div>
  );
}
