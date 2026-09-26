/**
 * The one type the segment dialog needs that is not a wire type.
 *
 * A separate file so the dialog can be read and tested without pulling in the whole IPC surface.
 */

import type { PresetWire } from "../ipc/types";

/** The preset fields the dialog shows. A subset of {@link PresetWire}, deliberately. */
export type PresetViewLite = Pick<PresetWire, "name" | "preservesPicture">;
