

Focus on clean changes that follow the project's overall design philosophy,
instead of making local changes until the tests fail.

Avoid fallbacks and heuristics if at all possible. If a bug can't be fixed without introducing
heavy special-purpose machinery, describe the problem to me in detail instead of introducing it.

Avoid changes to the renderer. We want to leave it as close to the default Typst renderer
as possible.

If necessary, call typst-diff with the --debug or --debug-trace options to get diagnostic information.

Distill nontrivial bugs into tests so they can be checked automatically.

Never read PDF files directly.

When diagnosing a complex bug, think through the program pipeline step by step
and formulate actionable expectations for the output of each step; then probe
whether the outputs match the expectations.
