# ADR-015: This build ships without a licensing feature

**Status:** accepted, superseding an earlier draft that specified one

**Context.** The product is intended for distribution and to carry a price, so an early draft of
this record specified an offline, signed licence file: a signature over a canonical payload naming
the licensee, the edition and an optional expiry, verified locally with an embedded key and no
network call. A `trimmer-license` crate was built to that specification and passed 54 tests.

The product owner then directed that this build must not include licensing. That direction is the
decision, and this record exists because the work was done and removed rather than never attempted,
and the reasoning is what a future reader needs.

**Decision.** No licensing feature. `crates/trimmer-license` is deleted, and every reference to it
is removed from the command line, the local API and every manifest. Nothing is stubbed or left
behind a feature flag:

* `thetrimmer` has no `licence` subcommand, and the `doctor` report has no licence line.
* `GET /v1/health` has no `licensed` field and `GET /v1/capabilities` has no `licence` object and no
  edition table.
* there is no `THE_TRIMMER_LICENCE` environment variable and no file it could name.

The tests assert the **absence** of those fields rather than merely not mentioning them. An empty
edition table is still a claim about what a caller is entitled to, and a claim with nothing behind
it is worse than no claim: a client that reads `features: []` learns something false.

**Consequences.** The application has no notion of entitlement, so it cannot gate a feature, count
seats, or refuse to run after an expiry. Distribution and pricing are therefore decided outside the
binary — by how it is sold and delivered rather than by anything it checks at startup. If licensing
is wanted later, the removed design is recoverable from this record and from the git history, and
the two properties that made it work offline are worth keeping: the signature is verified locally so
the tool works on a plane, and the private key never ships.

**Change in V2.** Recorded rather than assumed. The `docs/PRICING.md` that an earlier draft of the
README pointed at was never written and is not referenced now, because there is no in-product
entitlement for it to describe.