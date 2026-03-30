import { useEffect, useState, useRef } from "react";

interface TypeWriterProps {
  text: string;
  delay?: number;
  speed?: number;
  className?: string;
  onComplete?: () => void;
  showCursor?: boolean;
}

export default function TypeWriter({
  text,
  delay = 0,
  speed = 45,
  className = "",
  onComplete,
  showCursor = true,
}: TypeWriterProps) {
  const [displayedText, setDisplayedText] = useState("");
  const [isTyping, setIsTyping] = useState(false);
  const [isComplete, setIsComplete] = useState(false);
  const indexRef = useRef(0);

  useEffect(() => {
    // Reset state when text changes
    setDisplayedText("");
    setIsComplete(false);
    indexRef.current = 0;

    const startTimeout = setTimeout(() => {
      setIsTyping(true);
    }, delay);

    return () => clearTimeout(startTimeout);
  }, [text, delay]);

  useEffect(() => {
    if (!isTyping) return;

    if (indexRef.current >= text.length) {
      setIsTyping(false);
      setIsComplete(true);
      onComplete?.();
      return;
    }

    const timer = setTimeout(() => {
      setDisplayedText(text.slice(0, indexRef.current + 1));
      indexRef.current += 1;
    }, speed);

    return () => clearTimeout(timer);
  }, [isTyping, displayedText, text, speed, onComplete]);

  return (
    <span className={`launch-typewriter ${className}`}>
      <span className="launch-typewriter-text">{displayedText}</span>
      {showCursor && !isComplete && (
        <span className={`launch-typewriter-cursor ${isTyping ? "launch-typewriter-cursor--typing" : ""}`}>
          |
        </span>
      )}
    </span>
  );
}
