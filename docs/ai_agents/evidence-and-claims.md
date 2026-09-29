# Evidence and claims

The launcher owns the implementation in the client module and consumes
Bahamut's client contracts. Its code and tests do not by themselves prove retail
client behavior, server behavior, or success on every Wine and operating
system combination.

See the [policy index](README.md) and the [repository docs
index](../README.md).

## Evidence classes

Use the narrowest class that supports the claim:

| Class | Supports | Does not support |
|---|---|---|
| In-tree implementation | Current Rust, Tauri, configuration, workflow, and test behavior | Retail behavior or a live server result |
| Implemented tracked contract | Auth, handshake, configuration, and troubleshooting obligations tied to current code | Retail behavior or an obligation not implemented by the launcher |
| Exact-binary validation | Identity-gated bytes, offsets, signatures, or patch results checked against the named client build | Live client behavior, server behavior, or another client build |
| Unverified observation | A bounded uncertainty attached to a named surface | Shipped or supported behavior |
| Public format or protocol source | A clean room format rule or a published wire fact when the source is identified | An unverified interpretation of a client binary |
| Provenance and licensing record | Attribution, license compatibility, and the method used for the installer or launch backends | Runtime behavior |
| Reproducible observation | A named client build, OS, Wine engine, server, endpoint, or launch run under stated conditions | Behavior outside those conditions |
| Report or search lead | A question to investigate | A merged fact |

Repository code, tests, and docs are authoritative for the launcher's current
implementation contract. They are not retail evidence. Agent summaries,
search snippets, and unattributed statements remain leads until the underlying
artifact is inspected.

The tracked attribution record is [`LICENSE.md`](../../LICENSE.md). Use it for
provenance and licensing claims instead of repeating or paraphrasing its
records.

## Claims

Make the smallest claim the evidence supports. Distinguish these labels in
notes and reviews:

- `implemented` means the local code path exists and the applicable checks
  cover it.
- `contract` means a stable request, response, file, or launch shape owned by the
  launcher, documented in tracked docs, and tied to its implementation.
- `observed` means a named environment or run produced the result. State the
  client build and relevant platform or server conditions.
- `unverified` means the named evidence does not establish the behavior beyond
  the stated boundary.
- `provenance` means the statement explains source, attribution, or licensing.
  It does not prove behavior.

Do not describe a unit test as live retail validation. An optional test against
a retail binary pinned in the manifest supports only the exact static fact it checks.
Do not turn server behavior into a launcher claim merely because the
launcher sends the request that precedes it.

## Numbers in prose

Keep a figure only when the claim depends on it. Exact row counts, coverage
ratios, byte sizes, hashes, offsets, and extraction diffs are evidence and
stay verbatim.

Remove incidental figures. A useful rounded estimate may remain when precision
does not affect correctness. Label it as an estimate and name the source or
method when that context matters. Keep structural values and exact identities
exact. Figures inside quoted or transcribed source material stay verbatim.

## Citations

For a claim sourced from another public repository, link the stable public file
or section URL. For a claim sourced in this repository, use a stable tracked
path and a symbol, table row, or section locator when useful. For example:

```text
docs/handshake.md#pe-patches
```

Machine paths, ignored files, and unpublished sibling-repository shorthand are
not public citations. Keep the exact source and version text for patch RVAs,
client builds, protocol values, and license records.

If a claim changes an auth, handshake, configuration, or troubleshooting
contract, update the applicable tracked page and its index. Keep maintainer
reasoning, open research, and implementation history in the ignored island.
