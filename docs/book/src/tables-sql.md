# Tables & SQL

Table blocks use the **Vortex** columnar engine — chosen for *access*, not just size: O(1) random
`take`, column projection, filter pushdown, and zero-copy decode into Arrow. Rows are encoded in fixed
2¹⁶-row groups, so the same encoder handles a small table and a streamed >RAM one identically (the
batch and streaming paths are byte-for-byte equal).

For plain column reads use `tessera read` (see [Reading & navigating](./reading.md)); for analytical
queries, the `sql` sub-command runs DataFusion directly over a table block:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/sql.trycmd}}

`sql` is behind the `--features sql` build flag. Because Vortex decodes zero-copy to Arrow, the same
bytes are queryable from DuckDB / Polars / any Arrow consumer without a re-encode — the table block *is*
the query surface, not an export of one.
