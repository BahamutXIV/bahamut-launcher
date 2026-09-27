# AI-assisted contributions

[Back to the documentation index](../README.md)

AI-assisted work follows the same ownership standard as every other launcher
change. The contributor owns the result and explains its purpose, scope, and
verification.

Agent output is not evidence. Use [Evidence and claims](evidence-and-claims.md)
when turning research, observations, or reports into a claim.

## Documentation policy

### Public contract

Tracked prose describes the launcher's current contract. Do not record work
history, branch state, progress, or private maintainer context. Keep API
maturity, verification state, and compatibility limits when users need them to
interpret a contract safely.

Document only the current supported contract. If code accepts a compatibility
input, describe the input without its origin, old location, or prior behavior.
Do not add obsolete breadcrumbs or agent narration.

The launcher consumes Bahamut's client contracts. Keep server behavior in the
server repository. Describe only the launcher request, response, launch, and
handoff obligations here.

### Tracked docs

`README.md` and every tracked Markdown page under `docs/` are public docs.
They must stand on their own in a bare checkout.

Tracked docs and code must not link ignored maintainer material. Promote a
durable fact visible to users into the appropriate tracked page and add that page
to the applicable public index.

### Shape

Use ASCII punctuation, short paragraphs, and direct sentences. Use a list for
a real sequence and a table for repeated mappings. Lead with the subject rather
than describing what a page contains.
These are length and punctuation defaults for new prose. Preserve technical
identifiers and provenance when needed for clarity or correctness.

Link the canonical contract, source file, workflow, or technical guide instead
of copying details that can drift. Keep stable wire values, file shapes, and
evidence locators when they are part of the current launcher contract.

## Policy pages

- [Evidence and claims](evidence-and-claims.md) defines acceptable evidence,
  compatibility wording, and citation rules.
- [Comments and prose](comments-and-prose.md) defines the repository's source
  comment and public prose style.
