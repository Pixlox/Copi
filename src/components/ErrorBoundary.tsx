import { Component, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false, error: null };

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("[Copi] Render error:", error, info);
  }

  render() {
    if (this.state.hasError) {
      return (
        this.props.fallback ?? (
          <div
            style={{
              padding: "20px",
              color: "var(--danger-text, #ff6b6b)",
              background: "var(--danger-bg, rgba(255,0,0,0.05))",
              borderRadius: "8px",
              margin: "12px",
              fontSize: "12px",
            }}
          >
            Something went wrong. Press Escape to close and reopen Copi.
            <br />
            <code style={{ fontSize: "10px", opacity: 0.7 }}>
              {this.state.error?.message}
            </code>
          </div>
        )
      );
    }
    return this.props.children;
  }
}
