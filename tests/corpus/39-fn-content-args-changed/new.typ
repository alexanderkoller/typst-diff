// Function unchanged; content arguments differ from old version.

#let framed(title, body) = block(
  stroke: (thickness: 0.8pt),
  inset: (x: 10pt, y: 8pt),
  width: 100%,
  [*#title* #sym.dash.em #body],
)

#framed("Definition 1")[A graph is a set of nodes connected by edges.]

#framed("Definition 2")[A forest is a collection of disjoint acyclic graphs.]

#framed("Theorem")[Every finite connected graph contains a spanning tree as a subgraph.]
