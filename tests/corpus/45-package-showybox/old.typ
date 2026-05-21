// Uses @preview/showybox:2.0.4 from Typst Universe.
// In new.typ: box titles and body text change.
// Expected: modifications in the box content paragraphs.

#import "@preview/showybox:2.0.4": showybox

= Project Overview

#showybox(
  title: "Goal",
  frame: (
    border-color: blue,
    title-color: blue.lighten(30%),
  ),
)[
  The goal of this project is to analyze traffic patterns in urban areas
  and propose improvements to reduce congestion.
]

#showybox(
  title: "Scope",
  frame: (
    border-color: green,
    title-color: green.lighten(30%),
  ),
)[
  The study covers the downtown core, including all major intersections
  and arterial roads within a five-kilometer radius of city hall.
]

#showybox(
  title: "Timeline",
  frame: (
    border-color: orange,
    title-color: orange.lighten(30%),
  ),
)[
  Phase one will conclude by the end of March.
  Phase two deliverables are due in June.
]
