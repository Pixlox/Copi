interface ProgressWaveProps {
  progress: number; // 0 to 1
  indeterminate?: boolean;
  className?: string;
}

export default function ProgressWave({
  progress,
  indeterminate = false,
  className = "",
}: ProgressWaveProps) {
  return (
    <div className={`launch-progress ${className}`}>
      <div className="launch-progress-track">
        {indeterminate ? (
          <div className="launch-progress-wave launch-progress-wave--indeterminate" />
        ) : (
          <div
            className="launch-progress-fill"
            style={{ transform: `scaleX(${progress})` }}
          />
        )}
      </div>
      {/* Glow effect underneath */}
      <div
        className="launch-progress-glow"
        style={{ opacity: indeterminate ? 0.5 : progress * 0.7 }}
      />
    </div>
  );
}
