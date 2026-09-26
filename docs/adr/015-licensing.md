# ADR-015: Licensing is offline, signed and file-based

**Status:** accepted

**Context.** This is a product a studio pays for, and the people who pay for it work in cutting
rooms, on shared storage, and — often enough to matter — on machines that are not on the internet.
A plane, an offline edit suite, an air-gapped facility with a policy against outbound traffic. Any
licence scheme that requires a network round trip at launch is a scheme that stops the tool from
opening in exactly the places it is sold into.

That constrains the design more than it first appears. Online activation buys a great deal: it can
revoke, it can move a seat, it can cut off a leaked key within a minute, and it can tell you how
many installations exist. Offline verification buys none of that.

**Decision.** A licence is a **file**, and it is verified locally.

* The licence carries a canonical payload naming the licensee, the edition, an optional seat count
  and an optional expiry. Canonical means the bytes signed are a defined serialisation, not
  whatever `serde_json` happens to emit — a payload that reordered its object keys, or was
  re-serialised by a newer build, must still verify.
* The payload is signed with an Ed25519-style signature. Ed25519 rather than an HMAC because the
  verifier must be able to check a signature **without being able to make one**: an HMAC key that
  ships in the binary is a signing key that ships in the binary.
* The application verifies the signature with an **embedded public key** and never phones home. No
  activation server, no telemetry, no outbound call at launch, no call at all. `trimmer-media`
  documents the application as the only crate that starts a process, and a licensing check that
  made an HTTP request would quietly be a second kind of outbound activity; there is none.
* The licence is read from disk with a **size cap** and refused rather than parsed if it is
  absurdly large, because a file the user can point the app at is a file an attacker can shape.

**Consequences.** What it buys is a tool that opens on a plane, in an air-gapped facility, and on a
machine whose only network is the shared storage. It also buys a very small attack surface: no
server, no account system, no credential store, no session token, and no channel for a compromised
licence server to become a compromise of the product.

The trade is stated plainly because it is real: **offline verification cannot revoke.** A licence
that has been refunded keeps working until it expires, and one copied file works on every machine
it is copied to within its seat count's honest reading — there is nothing to check it against. This
is accepted. The alternative is a tool that stops working in a studio with no internet, which is a
worse failure than a refunded licence continuing to run, and it is a failure that lands on the
paying customer rather than on the person who took the refund. What mitigates it is expiry: a
licence with an end date is a licence that stops working on its own, so the exposure of any single
leak is bounded by the term rather than by forever. Editions without an expiry are therefore a
business decision with a security consequence, and should be priced and granted knowing that.

**The security model is key custody, and nothing else.** The signing key never ships — only its
public half. Everything above rests on the private key staying private: one leak and anyone can
mint a licence, and no verifying code can tell the difference, because a valid signature over a
valid payload *is* a valid licence. There is no revocation list to consult offline and no
counterparty to ask. So the private key's custody — where it lives, who can read it, how it is
backed up, and what happens if the person who holds it leaves — is the whole of the product's
licence security, and it deserves more care than any line of code in this document.

**What exists today.** Nothing. No licence code, no signature crate in the dependency table, and
no key material in the repository — which is the correct state for key material. This ADR records
the decision and its trade so that the implementation does not have to relitigate them; a
`thetrimmer licence` subcommand is documented in the README as part of the intended command
surface, not as shipped behaviour.
