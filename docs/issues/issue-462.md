---
type: issue
state: closed
created: 2026-09-28T21:12:00Z
updated: 2026-09-29T02:32:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/462
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:29.590Z
---

# [Issue 462]: [docs(cli): pin the inspect --json schema and close three provenance test gaps from the #453 review](https://github.com/vig-os/tessera/issues/462)

Follow-ups from the independent review of #453 (#416/#417). None block; all are docs or test depth.

- **Document the `inspect --json` schema.** It is exactly the serialized `Manifest`. Say that `producer` is a JSON *string* for a legacy `ProducerRef` and an *object* for a structured one (`#[serde(untagged)]`, manifest.rs:91-93), and whether its stability is tied to `tessera_version`.
- **Pin the `--json` output in `provenance.trycmd`.** It currently elides the output with `...`, so a legacy-producer or key-rename regression would not be caught.
- **Refuse a silent downgrade on resume.** `write.rs` `Header` has no `deny_unknown_fields`, so an older binary resuming a stage dir written by newer code drops producer/generation silently. That is the same loss class as #416. Either version the header or refuse unknown fields.
- **Test `generation_lines`** with `config_ref` **and** `--full` over the key cap together (nav.rs:418-477).

Refs: #453
