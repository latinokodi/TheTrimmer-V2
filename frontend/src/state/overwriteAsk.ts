/**
 * Whether a cut has to ask before it replaces something.
 *
 * ## Why this is a function and not two `useState` calls
 *
 * "Ask before overwriting" sounds like one condition and is really three: a plan that says the
 * output is occupied, a cut the engine has already refused for the same reason, and an operator
 * who has *already* said no to that particular file. The third is the one that is easy to lose.
 * A dialog with no memory of being dismissed reappears the instant it is dismissed — the plan is
 * still describing the same occupied path — and a question that cannot be answered is worse than
 * no question, because it looks like the program arguing.
 *
 * So the memory is part of the decision, and the decision is here, where it has tests. The
 * component draws it.
 *
 * ## Which file, not just whether
 *
 * The dialog names the file. That is what makes the answer possible: "overwrite?" is a question
 * about nothing in particular, and `Interview wide.mp4 already exists` is a question somebody can
 * answer. It also gives the dismissal something to be keyed on — declining one segment must not
 * silently decline the next one, which is what a bare `dismissed` boolean would do.
 *
 * ## Two moments, one question
 *
 * The plan knows the output is occupied before the button is pressed; the engine's refusal knows
 * it at the moment the cut is attempted, which is later and is the authority. A file can appear
 * in between — or be replaced by something else entirely — so the refusal is not a duplicate of
 * the plan, it is the same fact reported by the thing that has to act on it.
 */

/** A question waiting to be answered: the files a cut would replace. */
export interface OverwriteAsk {
  /** The path the cut would write, which is what a dismissal is recorded against. */
  readonly output: string;
  /** Their names, in the order the engine listed them. Never empty. */
  readonly files: readonly string[];
  /** Which moment found them, which is what the dialog's second line says. */
  readonly source: "planned" | "refused";
}

export interface OverwriteInput {
  /** The path this cut would write, as the engine resolves it. */
  readonly output: string;
  /**
   * The names the engine says are in the way: the plan's list, or the one that came with a refusal.
   *
   * One field rather than two, because they are the same list reported from two moments, and the
   * engine sends it in both answers. A second field would be a second place for it to be dropped.
   */
  readonly files: readonly string[];
  /** The output of a cut the engine has already refused, if it has refused one. */
  readonly refusedOutput: string | null;
  /** The engine's tag for that refusal. Only `"exists"` is a question about a file. */
  readonly refusedReason: string | null;
  /** Outputs the operator has already declined to replace. */
  readonly dismissed: ReadonlySet<string>;
}

/**
 * The question to put to the operator, or `null` when there is nothing to ask.
 *
 * The order matters: a refusal is newer than the plan and is about the cut that was actually
 * attempted, so it wins. Both paths end at the same place — an occupied output that nobody has
 * declined yet — and neither can ask about a file the engine did not name, because a question
 * with no filename in it is one nobody can answer.
 */
export function overwriteAsk(input: OverwriteInput): OverwriteAsk | null {
  const refused = input.refusedOutput !== null && input.refusedReason === "exists";
  const output = refused ? (input.refusedOutput as string) : input.output;
  const source: OverwriteAsk["source"] = refused ? "refused" : "planned";

  // A refusal that named no files cannot be asked about: there would be nothing to put in the
  // sentence. The engine always names them, so this is reached only by a malformed answer, and
  // inventing a filename to fill the gap would be worse than not asking.
  if (input.files.length === 0) {
    return null;
  }
  // Declining is about one file. This is the state that is easy to lose: without it the plan keeps
  // describing the same occupied path and the question comes straight back.
  if (input.dismissed.has(output)) {
    return null;
  }
  return { output, files: input.files, source };
}

/** The dialog's headline: one file or several, said in the number that is true. */
export function overwriteTitle(files: readonly string[]): string {
  return files.length === 1 ? "Replace this segment?" : `Replace these ${files.length} files?`;
}

/**
 * The dialog's second line, which has to be true at the moment it is shown.
 *
 * "already there" and "still there" are not decoration: the first is read before the button has
 * been pressed and the second after, at a moment when the operator has already been told once. A
 * single wording would be wrong in one of the two places.
 */
export function overwriteReason(source: OverwriteAsk["source"]): string {
  return source === "planned"
    ? "It is already there. Nothing is replaced without being asked first."
    : "It is still there. Nothing is replaced without being asked first.";
}
