# Comments and prose

The [documentation policy](README.md) is canonical for public prose. Apply
the same burden to Rust comments, Tauri and frontend comments, TOML comments,
build scripts, and workflow comments.

See the [repository docs index](../README.md).

Deletion is the default. These are defaults rather than hard length or
punctuation limits. Clarity, correctness, provenance, and technical identifiers
take precedence. Keep a comment only when it records one of these:

- a current invariant
- a client or platform quirk
- a wire or file-format fact
- an evidence or provenance citation
- a safety constraint
- an API, IPC, or serialization contract not inferable from types and names

Keep source identifiers, patch RVAs, protocol values, version stamps, and
license citations verbatim. They are evidence-bearing text, not narration to
shorten.

Compress other survivors to about one line at the use site. Move a longer
public contract to the relevant tracked page or API declaration and leave a
short pointer. Maintainer-only rationale may move to the ignored island, but
tracked docs and code must not point there. Remove branch-time plans, obsolete
status notes, historical rationale, and comments that repeat the next
statement.

Runtime strings shown to users are not comments. Command help, error text, log
messages exposed to operators, and visible HTML text are public behavior and
must be reviewed as behavior, not trimmed as prose.

Generated comments are generated output. Preserve them exactly, or update the
owning generator and regenerate the output.

When unsure, keep one short line and flag it in review notes. Never silently
delete a comment whose meaning may be a safety rule, a client constraint, or a
non-inferable contract.

## Examples

Keep a current safety rule:

```rust
// Plain HTTP is allowed only for loopback hosts.
```

Keep a contract pointer:

```rust
// The two launch patches are defined in docs/handshake.md.
```

Delete narration that repeats the code:

```rust
// Read the next entry.
let entry = entries.next();
```
