# Agent Trace 0.1.0 — pinned copy

This folder holds an unmodified copy of the JSON Schema from the
**Agent Trace** specification, Version 0.1.0 (Status: RFC, January 2026),
published by Cursor at <https://agent-trace.dev/>.

The specification is licensed under the Creative Commons Attribution 4.0
International License (CC BY 4.0): <https://creativecommons.org/licenses/by/4.0/>.
These files are redistributed under that license. No changes were made to
their content.

## Why it is pinned

The specification's source repository, <https://github.com/cursor/agent-trace>,
returned HTTP 404 on 2026-11-11, while agent-trace.dev stayed online. Until the
spec's status is clear, `prov` exports against this frozen copy, not the live site.

## Files

| File | Source | SHA-256 |
| --- | --- | --- |
| `trace-record.schema.json` | Byte-for-byte download of <https://agent-trace.dev/schemas/v1/trace-record.json> (the schema's own `$id`). Served as `application/schema+json`, ETag `"4aef8530aa3fd43ad4ef159e54e6b0aa"`, Last-Modified `Wed, 11 Nov 2026 02:07:56 GMT`. | `f86ded10e845445a21093283092752192b6c614093168035da93a47f696ca5eb` |
| `trace-record.spec-page.schema.json` | The schema as printed in the specification text at <https://agent-trace.dev/>, taken from the page's code block with the HTML markup removed. | `7988b6589a000410530000896a1454b2c394d75459726af38dd65e7ae462a39f` |

Both were retrieved on 2026-11-11.

## Known discrepancies

The two files describe the same record but are not equivalent:

- **Strictness.** The served file sets `additionalProperties: false` on every
  object, so it rejects unknown fields. The spec-page version does not.
- **Structure.** The served file is a generated, flattened form
  (`$ref: #/definitions/TraceRecord`). The spec-page version is hand-written
  with `$defs`.
- **Version string.** Both require `version` to match `^[0-9]+\.[0-9]+$`
  (for example `1.0`), yet the specification's own examples use `"0.1.0"`,
  which that pattern rejects.

`prov` should validate against `trace-record.schema.json` (the canonical `$id`
target) and record which `version` string it writes until the conflict is resolved.
