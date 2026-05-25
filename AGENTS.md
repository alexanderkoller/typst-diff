

Focus on clean changes that follow the project's overall design philosophy,
instead of making local changes until the tests fail.

Avoid fallbacks and heuristics if at all possible.

If necessary, write small probes for debugging.

Distill nontrivial bugs into tests so they can be checked automatically.

Never read PDF files directly.

When diagnosing a complex bug, think through the program pipeline step by step
and formulate actionable expectations for the output of each step; then probe
whether the outputs match the expectations.
