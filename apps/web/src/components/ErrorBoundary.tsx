/**
 * The crash boundary.
 *
 * A React render error in a desktop application is a blank window, and the user has no console and
 * no reload button. So the boundary catches, shows the error, and offers the two things that
 * actually help: a copy button, so the message can be pasted into a support request, and a reload.
 *
 * It deliberately does not try to recover by re-rendering. A render error means the component tree
 * disagrees with the state, and re-rendering the same tree produces the same error.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  readonly children: ReactNode;
}

interface State {
  readonly error: Error | null;
  readonly componentStack: string | null;
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ componentStack: info.componentStack ?? null });
    // The console is the only place a developer will look, so the full error goes there even
    // though the user only sees the message.
    console.error("TheTrimmer's interface failed to render", error, info);
  }

  override render(): ReactNode {
    const { error, componentStack } = this.state;
    if (error === null) {
      return this.props.children;
    }
    const detail = `${error.name}: ${error.message}${componentStack === null ? "" : `\n\n${componentStack}`}`;
    return (
      <div className="crash" role="alert">
        <h1 className="crash__title">The interface stopped</h1>
        <p className="crash__lead">
          This is a fault in TheTrimmer, not in your project. Nothing on disk was changed by it.
          Your cuts are saved as you make them, so reloading loses nothing.
        </p>
        <pre className="crash__detail selectable">{detail}</pre>
        <div className="crash__actions">
          <button
            type="button"
            className="btn"
            onClick={() => {
              void navigator.clipboard?.writeText(detail);
            }}
          >
            Copy the details
          </button>
          <button type="button" className="btn btn--primary" onClick={() => window.location.reload()}>
            Reload the window
          </button>
        </div>
      </div>
    );
  }
}
