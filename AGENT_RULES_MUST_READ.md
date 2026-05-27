1. Do NOT create backwards compatible indicies when asked to edit something. Replace the current index with a new schema that is non backwards compatible and requires a reindex.
2. If a new feature is requested ALWAYS prefer creating a new index for it if it means greatly improved read speeds
3. Any new JSON-RPC method, Oyl-compatible HTTP endpoint, or explorer API endpoint must be added to the explorer docs page at `src/explorer/pages/docs.rs` with an explanation, example query, and example response.
