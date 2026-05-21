// Uses @preview/cetz:0.3.2 from Typst Universe.
// In new.typ: node labels and a connecting edge change.
// Expected: modifications in the descriptive text; diagram itself is opaque.

#import "@preview/cetz:0.3.2": canvas, draw

= System Architecture

The diagram below shows the three-tier architecture of the application.

#align(center)[
  #canvas({
    import draw: *
    rect((-3, 1), (-1, 2), name: "client")
    content("client.center", [Client])
    rect((-0.5, 1), (1.5, 2), name: "server")
    content("server.center", [Server])
    rect((2, 1), (4, 2), name: "db")
    content("db.center", [Database])
    line("client.east", "server.west", mark: (end: ">"))
    line("server.east", "db.west", mark: (end: ">"))
  })
]

The client sends requests to the server, which queries the database.
All communication uses HTTP over TLS.

= Data Flow

Data enters through the client interface, is validated by the server,
and persisted in the relational database.
