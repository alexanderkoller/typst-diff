// Function body: bold only. Text is identical in both versions.
// Expected: all blocks Equal — the diff sees no textual change.

#let term(body) = text(weight: "bold", body)

The #term[initialisation] phase prepares the runtime environment.

During #term[execution] the scheduler dispatches work units to available workers.

The final #term[cleanup] phase releases locks and deallocates memory.
