# Browser foundation consumer contracts

The retained CSR gate builds `test/tools/web-bindings/fixtures/foundation-browser.rs`
with the local source components for `wasm32-unknown-unknown`. The existing
`web-production` → `browser-smoke` artifact path runs the generated bindings in
sandboxed Chrome. Native checks alone do not satisfy these browser contracts.

| Component | Targeted contract |
| --- | --- |
| next_tuple | Ordered extension and owned value preservation |
| slotmap | Removal invalidates a generation; secondary lookup rejects stale keys |
| or_poisoned | Actual Mutex/RwLock read and write guards retain values |
| const_str_slice_concat | Unicode concatenation, prefix/suffix, separated nonempty class tokens |
| throw_error | Nested error-hook scope restores the outer hook and clears by ID |
| console_error_panic_hook | Repeated installation produces one diagnostic for one intentional WASM panic |
| gloo-events | Event delivery, once-only delivery, and drop removal |
| gloo-timers | Timeout delivery/cancellation and interval cancellation |
| js-sys | Object property values, array length and object identity |
| web-sys | DOM text remains text, attributes round-trip, and event targets dispatch |

The intentional panic runs last. JavaScript requires both the matching console
diagnostic and a `WebAssembly.RuntimeError`; it does not treat a swallowed panic
as success. The browser runner rejects a report missing any of these ten checks.
This is targeted retained-consumer coverage, not a claim that every upstream API
or upstream suite has passed. Poison recovery is not provided or claimed.

The separator regression also runs natively with nonzero test counts, including
empty input, empty fragments, Unicode, multi-byte separators and an empty
separator. The original implementation returned `abc` for `["a", "", "bc"]`
joined with a space; the maintained implementation returns `a bc`. This affects
the real tuple template CLASS/STYLE consumer in view-renderer.

Run the established gates:

```sh
python3 .github/scripts/check-csr-scope.py
CHROME_BIN=/path/to/chrome CHROMEDRIVER=/path/to/chromedriver \
  python3 .github/scripts/check-csr-browser.py
```

The generated `target/csr-template-check/output/validation.json` includes the
foundation fixture digest and source/tree identity. `browser-result.json`
records executed checks and the actual browser version. Uncommitted-worktree
evidence is explicitly labeled and must not be substituted for final-head CI.

## Assistant-cloud preparation (2026-10-31)

The local full CSR scope check passed: reactive-state retained units 23 + 23,
CSR native fixtures 3, tracing native fixture 1, release WASM generation,
32 retired API rejections and 17 retired feature rejections. The dedicated
separator regression passed 2 tests after reproducing the original failure;
the Python scope/required-browser-result suite passed 7 tests.

The browser did **not** execute in this local run. Chromium 155.0.8059.79 with
its matching driver exited because the execution sandbox denied its required
Unix socket. A standard escalated launch also failed in the environment
wrapper before the browser started. Browser sandboxing was not disabled.
Consequently these ten WASM consumer entries remain pending until the existing
GitHub browser-smoke job emits a successful complete browser result for the
integrated candidate. Compilation and native results must not be counted as
browser execution.
