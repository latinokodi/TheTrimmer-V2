/**
 * What the segment-name row says, as a function rather than as conditionals in the markup.
 *
 * This exists because the row got it wrong in a way that was invisible in the code and obvious to
 * the person using it. The first version derived the path from the *plan*, the plan does not run
 * until both marks are set, and the row read an empty path as "the engine refused this name" — so
 * every name typed before a range was marked was reported as unusable, next to a message pointing
 * at a reason that was printed nowhere.
 *
 * Three states, and only one of them is a refusal:
 *
 *   * nothing typed                 — say what an empty box does;
 *   * typed, and the engine answered — say where the segment will go;
 *   * typed, and the engine refused  — say why, in the engine's own words.
 *
 * "Typed and not yet answered" is the state that was missing, and it is why this is a function:
 * a state nobody named is a state nobody handled.
 */

export interface NameRow {
  /** The text to show in the row. */
  readonly text: string;
  /** Whether to draw it as a problem. Only ever true for a refusal from the engine. */
  readonly problem: boolean;
}

export interface NameState {
  /** What is in the name field, untrimmed. */
  readonly typed: string;
  /** Where the engine says the segment will be written, or "" when it has not said yet. */
  readonly output: string;
  /** The engine's sentence when it refused the name, or null when it did not. */
  readonly refused: string | null;
  /** Whether a source is loaded at all: with none, no name can be resolved. */
  readonly hasSource: boolean;
}

export function nameRow(state: NameState): NameRow {
  const typed = state.typed.trim();

  // A refusal is only ever what the engine said. Nothing here infers one.
  if (state.refused !== null) {
    return { text: state.refused, problem: true };
  }

  if (typed === "") {
    return { text: "leave empty for the range name", problem: false };
  }

  if (state.output !== "") {
    return { text: state.output, problem: false };
  }

  // Typed, nothing refused, and no path yet. Either the answer is still on its way or there is no
  // source to resolve it against — and in neither case has anything been refused. Saying nothing is
  // the only honest thing available, so it says nothing.
  return { text: state.hasSource ? "…" : "load a file to name its segments", problem: false };
}
