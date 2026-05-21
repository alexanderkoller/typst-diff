// Uses @preview/cetz:0.3.2 from Typst Universe.
// Node labels and descriptive text changed from old.typ.

#import "@preview/cetz:0.3.2": canvas, draw

= System Architecture

The diagram below shows the four-tier architecture of the application.

#align(center)[
  #canvas({
    import draw: *
    rect((-4.5, 1), (-2.5, 2), name: "client")
    content("client.center", [Browser])
    rect((-2, 1), (0, 2), name: "gateway")
    content("gateway.center", [Gateway])
    rect((0.5, 1), (2.5, 2), name: "server")
    content("server.center", [API])
    rect((3, 1), (5, 2), name: "db")
    content("db.center", [Cache])
    line("client.east", "gateway.west", mark: (end: ">"))
    line("gateway.east", "server.west", mark: (end: ">"))
    line("server.east", "db.west", mark: (end: ">"))
  })
]

The browser sends requests to the gateway, which routes them to the API service.
All communication uses gRPC over mTLS.

= Data Flow

Data enters through the browser interface, is authenticated by the gateway,
processed by the API, and cached for subsequent requests.
