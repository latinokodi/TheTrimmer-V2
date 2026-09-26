# ADR-013: Presets are data, and a preset that reshapes the frame forfeits passthrough

**Status:** accepted

**Context.** V1 had two knobs: `--crf` and `--preset`. That is the right shape for a utility and
the wrong shape for a product, because the question an editor actually asks is not "give me a CRF"
— it is "give me the vertical cut and the YouTube cut from the same segment". The V1 answer was to
run the trim twice with different flags, which is a second full pass over the source and a second
chance to get the marks wrong.

The other half of the context is the thing the product is *for*. The head patch is cheap because
the body is a stream copy: the original packets, bit for bit, and no encode at all. Anything that
changes the picture — a crop to vertical, a scale to 1920×1080, a different codec — destroys that.
A preset that asks for it is asking for a full re-encode of the whole segment, which on a
two-hour source is the hour-long job the head patch exists to avoid.

**Decision.** A preset is a *value*: name, description, container, video treatment, audio
treatment, frame geometry, loudness target, default handle frames, and whether it is safe to run
unattended as a batch. It is data, and adding a format is adding a row rather than adding a code
path. A project names a default preset, a segment can override it, and an export carries it.

The rule that matters is checked before anything runs. `DeliveryPreset::preserves_picture` is true
only when the video treatment is `Copy` **and** the geometry does not force an encode for the
source's dimensions. `CutExecutor::cut_with_plan` asks it *first* — before the mode match, before
any command is built — and routes a preset that cannot preserve the picture to the whole-segment
re-encode path. It does not silently degrade a passthrough request into a transcode, and it does
not silently ignore a crop because the cut was going to be cheap. It says so out loud, on the
progress sink:

> the `vertical` preset changes the picture, so the whole segment is re-encoded rather than copied

The check is made *before the job starts* rather than discovered per segment, which is the point.
A hundred-segment batch on `vertical` is a hundred full transcodes — hours of machine time — and
the operator finds out when they ask for it, not when the queue is eighty items in and the disk is
full. `preset_forces_full_encode(media, preset)` is the planner-side spelling of the same question,
for a caller that wants to warn before building a queue at all; it is deliberately a thin negation
of `preserves_picture` rather than a second rule that could drift from the executor's.
`DeliveryPreset::validate` refuses combinations that cannot be produced at all (a video treatment
in a container that carries no picture, a re-encode with no encoder, a CRF above 51, an impossible
sample rate) before a command line is built. Each preset also carries `batch_safe`, which is false
for the ones that are too slow or too consequential to run unattended — `prores_master`,
`broadcast_r128` and `mxf_op1a`.

Loudness follows the same logic. A cut taken out of the middle of a podcast has the level of the
room it was recorded in, so a preset states a target and a measurement basis, and the re-encode
path hands them to ffmpeg's `loudnorm`; a preset with no loudness target leaves the audio alone,
which is what a master delivery wants. Loudness normalisation requires an audio re-encode, so
`is_passthrough` — which is `video.is_copy() && audio == Copy` — is the honest report of whether
anything at all is being re-encoded. Note the asymmetry, because it is deliberate and worth not
tripping over: `preserves_picture` answers a *picture* question and so considers geometry but not
loudness; `head_patch` is reached only when `preserves_picture` is true, and it never consults the
preset's loudness target. A preset that copies the picture and normalises the audio would be
routed to the head patch and would not get its normalisation.

**Consequences.** The standard library is eleven presets — `master`, `master_faststart`,
`prores_master`, `youtube_1080`, `vertical`, `vertical_blur`, `square_social`, `podcast_audio`,
`wav_split`, `broadcast_r128`, `mxf_op1a` — and only three of them (`master`, `master_faststart`,
`wav_split`) pass both picture and sound through untouched. That is not a defect; it is the honest
count of how many of those formats can be delivered without re-encoding the picture. The rest
exist so that the same segment can go to a platform without a second pass in another app, and the
cost of each is visible before it starts.

The cost of the design is that a preset is a fairly wide struct, and every consumer of a plan has
to be aware that the preset can change what the plan means — a `HeadPatch` plan under a
geometry-forcing preset becomes a `Reencode` plan. That is a real complication, and it is in
exchange for the alternative: a tool that quietly transcodes a hundred segments, or one that
quietly ignores the crop the operator asked for.
