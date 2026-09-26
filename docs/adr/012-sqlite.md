# ADR-012: SQLite for projects, and why not a document file

**Status:** accepted

**Context.** V2 has projects. V1 did not: it took a file and two timecodes, cut once, and wrote a
log. A project is a source set, a segment list, the preset each segment delivers as, a record of
the cut runs that have happened, and an audit trail of who did what — and it lives for months,
across application upgrades, and across more than one machine.

The obvious alternative is a JSON document beside the media. It is human-readable, trivially
diffable, needs no dependency, and would have been the right answer for V1's one-shot trim. It is
the wrong answer here for one specific reason: **the data is relational and the relations are
load-bearing.** An audit entry names the cut run it belongs to; a cut run names the source it read
and the segment it wrote. In a document those are referenced by hand, and a hand-maintained
reference is a reference that can point at something that is not there. In a database they are a
foreign key, and the file refuses to hold a dangling one.

JSON also has no story for concurrent writers, no story for a partial write, and no story for
"open the project written by 2.0 in 2.1" beyond "hope the unknown fields are tolerated". An
audit log that can be half-written is not an audit log.

**Decision.** `trimmer-store` holds projects in SQLite via `rusqlite` with the bundled build. The
tables are the nouns: sources, segments, cut runs, and an audit log, with a foreign key from each
run to the source it read. On every connection:

* **Foreign keys are enabled** (`PRAGMA foreign_keys = ON`). SQLite defaults them off, which
  makes the relational argument above free of charge and worth nothing.
* **WAL journalling** is on, so a reader — the workspace UI, a studio's query — is not blocked by
  a writer, and an interrupted write does not leave a torn file.
* **A schema-version table with forward-only migrations.** Each migration moves the file one
  version forward, and a file whose version is newer than the binary supports is refused with a
  sentence saying so rather than opened and misread. Forward-only is the point: a migration that
  can go both ways has two code paths, one of which is never exercised and is therefore wrong.

**Indexes exist only on the two access paths that are real** — loading a project's segments, and
listing a project's runs in time order. Every other index would be a guess about a query nobody
has written, paid for on every insert.

**Consequences.** What it buys is the guarantee that a project cannot reference a source it does
not hold, a single-file format that survives a crash mid-write, and an upgrade path that is a
number in a table rather than a migration script somebody runs by hand. It also means a studio
that wants to answer "which of our masters have we cut from this month" can open the file with
any SQLite client.

What it costs is stated plainly, because it is a real loss: **a SQLite file is harder to hand-edit
and much harder to diff than JSON.** `git diff` on it is noise. Two people cannot merge their
changes to one project, and a studio cannot review a project in a pull request. That is accepted
for the live project file, and it is why the application owes the user an **export to a readable
form** — the project as JSON or CSV, versionable and reviewable, with the database as the working
copy rather than the record. The export is part of this decision, not a nicety attached to it.

**What exists today.** Nothing. `rusqlite` is declared in the workspace's dependency table, but
`crates/trimmer-store` is a commented-out workspace member and the crate does not exist. What does
exist is the seam the store will implement: `trimmer-app`'s `ProjectStore` trait — `save`, `load`,
`list`, `delete` — deliberately "a place to put a project and get it back; anything cleverer
belongs in the application layer, where it can be tested without a database". Any implementation
of that trait can be substituted in a test, which is what keeps the queue's tests free of SQLite.
