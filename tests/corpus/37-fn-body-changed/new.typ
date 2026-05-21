// Function body: bold + italic + blue. Text is identical to old version.
// Expected: all blocks Equal — same plain text, only styling differs.

#let term(body) = text(weight: "bold", style: "italic", fill: blue.darken(20%), body)

The #term[initialisation] phase prepares the runtime environment.

During #term[execution] the scheduler dispatches work units to available workers.

The final #term[cleanup] phase releases locks and deallocates memory.
