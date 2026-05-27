# TODO

8. Add a deterministic reorg regression harness that indexes a fork, switches to a competing canonical branch, and asserts that:
   - every enabled module rolls back to the parent height before replacement blocks run;
   - `StateAt::Latest` reads from the rewound versioned-tree root during replacement indexing;
   - orphaned height-to-blockhash mappings are no longer exposed as canonical;
   - missing canonical Alkane traces fail the block instead of silently advancing.
