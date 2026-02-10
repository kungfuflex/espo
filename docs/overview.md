The presence of block header data in earlier traces is expected; that alone doesn’t trigger the bug.
The cleaner only runs when a trace has `returns > invokes` (i.e., extra returns). If a trace already
has matching counts, it isn’t modified at all.

The bug only shows up when BOTH of these are true:
1) There is a return mismatch (`returns > invokes`).
2) There is more than one “host-like” return (success, empty alkanes/storage, data == header/coinbase/diesel/fee),
   including a real contract return that happens to equal a host value.

In that case, the old cleaner could drop the wrong return because it chose candidates without
respecting stack depth. If it removed a real contract return instead of the extra host return, the
invoke/return pairing shifted and you got the exact `(incoming - returned)` surplus.

So earlier blocks can have the same *shape* and even host-value returns, but still index fine if:
- `returns == invokes`, or
- the only host-like returns were actual host returns at stack edges, or
- there was no collision where a contract return’s data equaled a host value.

This explains why it only manifested on these txids despite similar-looking traces before.
