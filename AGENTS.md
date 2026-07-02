

Focus on clean changes that follow the project's overall design philosophy,
instead of making local changes until the tests fail.

Avoid fallbacks and heuristics if at all possible. If a bug can't be fixed without introducing
heavy special-purpose machinery, describe the problem to me in detail instead of introducing it.

Avoid code that makes post-hoc guesses about the document structure, content, or element provenance,
when this information was available to the program at an earlier time. Prefer to retain that
information and make clean decisions, rather than adding heuristics.

Avoid changes to the renderer. We want to leave it as close to the default Typst renderer
as possible.

If necessary, call typst-diff with the --debug or --debug-trace options to get diagnostic information.

Distill nontrivial bugs into tests so they can be checked automatically.

Never read PDF files directly.

When diagnosing a complex bug, think through the program pipeline step by step
and formulate actionable expectations for the output of each step; then probe
whether the outputs match the expectations.

Never update the reference for the corpus tests - let me do it myself.

After every major change to the codebase, summarize your technical decisions (and the justifications and
tradeoffs that went into them) in TECHNICAL-DECISIONS.md. Read this document at the start of each
conversation so you can make consistent technical decisions.

If you find that diagnosing a bug takes more than two minutes, you should first replicate this
bug with a minimal example and convert it into a failing test. Continue looking for solutions, but
only after you have isolated the problem into a test that fails for the right reasons.
