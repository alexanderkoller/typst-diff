// Function unchanged; only content arguments differ.
// Expected: word-level diff shows "vertices"→"nodes" and "tree"→"forest".

#let framed(title, body) = block(
  stroke: (thickness: 0.8pt),
  inset: (x: 10pt, y: 8pt),
  width: 100%,
  [*#title* #sym.dash.em #body],
)

#framed("Definition 1")[A graph is a set of vertices connected by edges.]

#framed("Definition 2")[A tree is a connected acyclic graph with exactly n-1 edges.]

#framed("Theorem")[Every finite connected graph contains a spanning tree as a subgraph.]
